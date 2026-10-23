//! Shared, live todo list: htmloxide (async components) + htmx + Server-Sent Events.
//!
//! [htmx](https://htmx.org) is a small JavaScript library: HTML attributes such
//! as `hx-post` send requests, and HTML that comes back is swapped into the
//! page. Server-Sent Events (SSE) are a long-lived HTTP response through which
//! the server pushes messages to the browser. Together they make a page that
//! updates live with no hand-written JavaScript.
//!
//! * Every mutation is a plain `hx-post` / `hx-delete` with `hx-swap="none"`;
//!   the handler just changes state and broadcasts that something changed.
//! * Every open tab holds one SSE connection (`/events`). On each broadcast the
//!   server re-renders the affected component and pushes the HTML as a named
//!   event; htmx swaps it into the element with the matching `sse-swap`.
//! * A clock (every second) and a live viewer count use the same channel.
//!
//! Run with `cargo run -p live-todos`, open http://127.0.0.1:3000 in two tabs.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::{delete, get, post};
use futures_util::stream::{self, Stream, StreamExt};
use htmloxide::{Markup, component, each, html, html_async};
use serde::Deserialize;
use tokio::sync::{RwLock, broadcast};
use tokio_stream::wrappers::{BroadcastStream, IntervalStream};

// --------------------------------------------------
// Data: todos and their store
// --------------------------------------------------

#[derive(Clone)]
struct Todo {
    id: u32,
    title: String,
    done: bool,
}

/// Stand-in for a database: every call is async, and `list` is a little slow,
/// like a real query.
struct Store {
    todos: RwLock<Vec<Todo>>,
    next_id: AtomicU32,
}

impl Store {
    async fn list(&self) -> Vec<Todo> {
        tokio::time::sleep(Duration::from_millis(5)).await;
        self.todos.read().await.clone()
    }
    async fn add(&self, title: String) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.todos.write().await.push(Todo {
            id,
            title,
            done: false,
        });
    }
    // Returns `false` when no todo has this id.
    async fn toggle(&self, id: u32) -> bool {
        let mut todos = self.todos.write().await;
        todos
            .iter_mut()
            .find(|t| t.id == id)
            .map(|t| t.done = !t.done)
            .is_some()
    }
    // Returns `false` when no todo has this id.
    async fn remove(&self, id: u32) -> bool {
        let mut todos = self.todos.write().await;
        let before = todos.len();
        todos.retain(|t| t.id != id);
        todos.len() != before
    }
}

// --------------------------------------------------
// Data: app state and update messages
// --------------------------------------------------

/// What changed; each SSE client re-renders the matching fragment.
#[derive(Clone, Copy, Debug)]
enum Update {
    Todos,
    Viewers,
    Clock,
}

/// Everything the handlers share. `updates` is a broadcast channel: every
/// `send` reaches every open SSE connection.
struct AppState {
    store: Store,
    viewers: AtomicUsize,
    updates: broadcast::Sender<Update>,
}

type Shared = Arc<AppState>;

// --------------------------------------------------
// Component: Layout
// --------------------------------------------------

const CSS: &str = r#"
  body { font-family: system-ui, sans-serif; max-width: 34rem; margin: 3rem auto; padding: 0 1rem; color: #222; }
  header { display: flex; justify-content: space-between; align-items: baseline; gap: 1rem; }
  .meta { color: #777; font-size: .9rem; font-variant-numeric: tabular-nums; }
  form { display: flex; gap: .5rem; margin: 1.5rem 0; }
  form input { flex: 1; padding: .5rem; font: inherit; }
  ul { list-style: none; padding: 0; }
  li { display: flex; align-items: center; gap: .6rem; padding: .45rem 0; border-bottom: 1px solid #eee; }
  li.done span { text-decoration: line-through; color: #999; }
  li button { margin-left: auto; border: none; background: none; cursor: pointer; color: #b33; font-size: 1.1rem; }
  .empty, .hint { color: #888; }
  .htmx-settling li { transition: background .4s; }
"#;

// The page shell: loads htmx and its SSE extension, and opens the SSE
// connection on `<body>`. `PreEscaped` writes the CSS without escaping.
#[component]
fn Layout(#[prop(into)] title: String, children: Markup) -> Markup {
    html! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>{&title}</title>
                <script src="https://cdn.jsdelivr.net/npm/htmx.org@2.0.10/dist/htmx.min.js"></script>
                <script src="https://cdn.jsdelivr.net/npm/htmx-ext-sse@2.2.4"></script>
                <style>{htmloxide::PreEscaped(CSS)}</style>
            </head>
            // One EventSource for the whole page; children pick events by name.
            <body hx-ext="sse" sse-connect="/events">
                {children}
            </body>
        </html>
    }
}

// --------------------------------------------------
// Component: Clock
// --------------------------------------------------

// The current UTC time, worked out from the Unix timestamp. Re-rendered and
// pushed to every tab once a second.
#[component]
fn Clock() -> Markup {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    html! { <span>server time {format!("{h:02}:{m:02}:{s:02}")} UTC</span> }
}

// --------------------------------------------------
// Component: Viewers
// --------------------------------------------------

// How many tabs are connected, i.e. how many SSE connections are open.
#[component]
fn Viewers(count: usize) -> Markup {
    let label = if count == 1 { "viewer" } else { "viewers" };
    html! { <span>{count} {label}</span> }
}

// --------------------------------------------------
// Component: TodoItem
// --------------------------------------------------

// One todo. The checkbox and the delete button send their request and ignore
// the response (`hx-swap="none"`): the new list arrives over SSE instead,
// in this tab and in every other one.
#[component]
fn TodoItem(todo: &Todo) -> Markup {
    html! {
        <li class={todo.done.then_some("done")}>
            <input type="checkbox" checked={todo.done}
                hx-post={format!("/todos/{}/toggle", todo.id)} hx-swap="none">
            <span>{&todo.title}</span>
            <button hx-delete={format!("/todos/{}", todo.id)} hx-swap="none" aria-label="Delete">"×"</button>
        </li>
    }
}

// --------------------------------------------------
// Component: TodoList
// --------------------------------------------------

/// Async: reads from the store where the data is actually needed.
#[component]
async fn TodoList(store: &Store) -> Markup {
    let todos = store.list().await;
    let left = todos.iter().filter(|t| !t.done).count();
    if todos.is_empty() {
        return html! { <p class="empty">Nothing to do. Add something above.</p> };
    }
    html! {
        <ul>{each(todos.iter().map(|todo| html! { <TodoItem todo={todo} /> }))}</ul>
        <p class="meta">{left} of {todos.len()} left</p>
    }
}

// --------------------------------------------------
// Component: Page
// --------------------------------------------------

// The whole page. Each `sse-swap="name"` element has its content replaced
// whenever an SSE event with that name arrives. The form clears itself after
// a successful post (`hx-on::after-request`).
#[component]
async fn Page(state: &AppState) -> Markup {
    let store = &state.store;
    let viewers = state.viewers.load(Ordering::Relaxed);
    html_async! {
        <Layout title="Shared todos">
            <header>
                <h1>Shared todos</h1>
                <div class="meta">
                    <div sse-swap="clock"><Clock /></div>
                    <div sse-swap="viewers"><Viewers count={viewers} /></div>
                </div>
            </header>

            <form hx-post="/todos" hx-swap="none" hx-on::after-request="if (event.detail.successful) this.reset()">
                <input name="title" placeholder="What needs doing?" required autofocus autocomplete="off">
                <button>Add</button>
            </form>

            <div id="todos" sse-swap="todos"><TodoList store={store} /></div>

            <p class="hint">Open this page in a second tab: changes show up in both, pushed over SSE.</p>
        </Layout>
    }
    .await
}

// --------------------------------------------------
// Handler: index
// --------------------------------------------------

// `GET /`: the full page. `html_async!` is an `async move` block, so the
// `AppState` is borrowed out of the `Arc` first and the block captures the
// reference.
async fn index(State(state): State<Shared>) -> Html<String> {
    let state = &*state;
    Html(html_async! { <Page state={state} /> }.await.into_string())
}

// --------------------------------------------------
// Handler: add_todo
// --------------------------------------------------
//
// The three todo handlers follow the same shape: change the store, broadcast
// `Update::Todos`, and answer with an empty `204 No Content`. `send` only
// fails when no tab is connected, which is fine, so its result is ignored.

/// The form body of `POST /todos`.
#[derive(Deserialize)]
struct NewTodo {
    title: String,
}

async fn add_todo(State(state): State<Shared>, Form(form): Form<NewTodo>) -> StatusCode {
    let title = form.title.trim();
    if title.is_empty() {
        return StatusCode::UNPROCESSABLE_ENTITY;
    }
    state.store.add(title.to_owned()).await;
    let _ = state.updates.send(Update::Todos);
    StatusCode::NO_CONTENT
}

// --------------------------------------------------
// Handler: toggle_todo
// --------------------------------------------------

async fn toggle_todo(State(state): State<Shared>, Path(id): Path<u32>) -> StatusCode {
    if !state.store.toggle(id).await {
        return StatusCode::NOT_FOUND;
    }
    let _ = state.updates.send(Update::Todos);
    StatusCode::NO_CONTENT
}

// --------------------------------------------------
// Handler: delete_todo
// --------------------------------------------------

async fn delete_todo(State(state): State<Shared>, Path(id): Path<u32>) -> StatusCode {
    if !state.store.remove(id).await {
        return StatusCode::NOT_FOUND;
    }
    let _ = state.updates.send(Update::Todos);
    StatusCode::NO_CONTENT
}

// --------------------------------------------------
// Handler: events
// --------------------------------------------------
//
// `GET /events`: one long-lived SSE response per open tab. It merges the
// broadcast updates with a one-second clock tick, renders the component that
// matches each update, and sends the HTML as an event named after it.

/// Decrements the viewer count when an SSE connection's stream is dropped.
struct ViewerGuard(Shared);
impl ViewerGuard {
    fn new(state: Shared) -> Self {
        state.viewers.fetch_add(1, Ordering::Relaxed);
        let _ = state.updates.send(Update::Viewers);
        Self(state)
    }
}
impl Drop for ViewerGuard {
    fn drop(&mut self) {
        self.0.viewers.fetch_sub(1, Ordering::Relaxed);
        let _ = self.0.updates.send(Update::Viewers);
    }
}

/// Renders the component for `update` as an SSE event. The event name
/// matches the `sse-swap` attribute of the element it replaces.
async fn render_update(state: &AppState, update: Update) -> Event {
    let (name, markup) = match update {
        Update::Todos => {
            let store = &state.store;
            ("todos", html_async! { <TodoList store={store} /> }.await)
        }
        Update::Viewers => {
            let count = state.viewers.load(Ordering::Relaxed);
            ("viewers", html! { <Viewers count={count} /> })
        }
        Update::Clock => ("clock", html! { <Clock /> }),
    };
    Event::default().event(name).data(markup.into_string())
}

async fn events(State(state): State<Shared>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Subscribe first, so this client also receives its own "viewers" bump.
    let updates = BroadcastStream::new(state.updates.subscribe())
        // A lagging client missed something: just refresh the list.
        .map(|msg| msg.unwrap_or(Update::Todos));
    let guard = ViewerGuard::new(state.clone());

    let ticks =
        IntervalStream::new(tokio::time::interval(Duration::from_secs(1))).map(|_| Update::Clock);

    // `select` merges both streams into one; `then` renders each update.
    let stream = stream::select(updates, ticks).then(move |update| {
        let state = state.clone();
        async move { render_update(&state, update).await }
    });

    // The guard lives as long as the stream, i.e. the connection.
    let stream = stream.map(move |event| {
        let _keep_alive = &guard;
        Ok(event)
    });

    // `KeepAlive` sends a comment line now and then, so idle connections
    // aren't closed along the way.
    Sse::new(stream).keep_alive(KeepAlive::default())
}

// --------------------------------------------------
// Handler: not_found
// --------------------------------------------------

async fn not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Html(html! { <h1>Not found</h1> }.into_string()),
    )
}

// --------------------------------------------------
// Entry point: main
// --------------------------------------------------

#[tokio::main]
async fn main() {
    // 64 is how far a slow tab can fall behind before it misses updates. The
    // receiver returned here is dropped: each SSE connection subscribes its
    // own.
    let (updates, _) = broadcast::channel(64);
    let state = Arc::new(AppState {
        store: Store {
            todos: RwLock::new(vec![
                Todo {
                    id: 1,
                    title: "Try htmloxide async components".into(),
                    done: true,
                },
                Todo {
                    id: 2,
                    title: "Open a second tab".into(),
                    done: false,
                },
            ]),
            next_id: AtomicU32::new(3),
        },
        viewers: AtomicUsize::new(0),
        updates,
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/events", get(events))
        .route("/todos", post(add_todo))
        .route("/todos/{id}/toggle", post(toggle_todo))
        .route("/todos/{id}", delete(delete_todo))
        .fallback(not_found)
        .with_state(state);

    // `ADDR=0.0.0.0:8080 cargo run -p live-todos` listens elsewhere.
    let addr = std::env::var("ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    println!("listening on http://{addr}");
    // Ctrl+C stops the server cleanly.
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await
        .expect("server");
}
