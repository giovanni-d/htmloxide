//! A shared todo list that updates live in every open tab, built with
//! htmloxide, htmx and Server-Sent Events. Run with `cargo run -p live-todos`
//! and open http://127.0.0.1:3000 in two tabs.
//!
//! [htmx](https://htmx.org) is a JavaScript library driven by HTML
//! attributes: `hx-post` sends a request, and HTML that comes back is swapped
//! into the page. Server-Sent Events (SSE) are a long-lived HTTP response
//! through which the server pushes messages to the browser.
//!
//! Each change is a plain `hx-post` / `hx-delete` whose response is ignored
//! (`hx-swap="none"`); the handler changes the state and broadcasts what
//! changed. Each tab holds one SSE connection, on which the server pushes
//! the re-rendered component as an event named after it, and htmx swaps it
//! into the element whose `sse-swap` has that name.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{delete, get, post};
use futures_util::stream::{self, Stream, StreamExt};
use htmloxide::{Markup, component, html, html_async};
use serde::Deserialize;
use tokio::sync::{RwLock, broadcast};
use tokio_stream::wrappers::{BroadcastStream, IntervalStream};

#[derive(Clone)]
struct Todo {
    id: u32,
    title: String,
    done: bool,
}

/// Stands in for a database: every call is async, and `list` is slow like a
/// real query.
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
    async fn toggle(&self, id: u32) -> bool {
        let mut todos = self.todos.write().await;
        todos
            .iter_mut()
            .find(|t| t.id == id)
            .map(|t| t.done = !t.done)
            .is_some()
    }
    async fn remove(&self, id: u32) -> bool {
        let mut todos = self.todos.write().await;
        let before = todos.len();
        todos.retain(|t| t.id != id);
        todos.len() != before
    }
}

#[derive(Clone, Copy, Debug)]
enum Update {
    Todos,
    Viewers,
    Clock,
}

struct AppState {
    store: Store,
    viewers: AtomicUsize,
    updates: broadcast::Sender<Update>,
}

type Shared = Arc<AppState>;

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

#[component]
fn Layout(#[prop(into)] title: String, children: Markup) -> Markup {
    html! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>{title}</title>
                <script src="https://cdn.jsdelivr.net/npm/htmx.org@2.0.10/dist/htmx.min.js"></script>
                <script src="https://cdn.jsdelivr.net/npm/htmx-ext-sse@2.2.4"></script>
                <style>{CSS}</style>
            </head>
            // One SSE connection for the whole page.
            <body hx-ext="sse" sse-connect="/events">
                {children}
            </body>
        </html>
    }
}

#[component]
fn Clock() -> Markup {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    html! { <span>server time {format!("{h:02}:{m:02}:{s:02}")} UTC</span> }
}

#[component]
fn Viewers(count: usize) -> Markup {
    let label = if count == 1 { "viewer" } else { "viewers" };
    html! { <span>{count} {label}</span> }
}

#[component]
fn TodoItem(todo: &Todo) -> Markup {
    html! {
        <li class={todo.done.then_some("done")}>
            <input type="checkbox" checked={todo.done}
                hx-post={format!("/todos/{}/toggle", todo.id)} hx-swap="none">
            <span>{todo.title}</span>
            <button hx-delete={format!("/todos/{}", todo.id)} hx-swap="none" aria-label="Delete">x</button>
        </li>
    }
}

#[component]
async fn TodoList(store: &Store) -> Markup {
    let todos = store.list().await;
    let left = todos.iter().filter(|t| !t.done).count();
    if todos.is_empty() {
        return html! { <p class="empty">Nothing to do. Add something above.</p> };
    }
    html! {
        <ul>@for todo in &todos { <TodoItem {todo} /> }</ul>
        <p class="meta">{left} of {todos.len()} left</p>
    }
}

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

            <div id="todos" sse-swap="todos"><TodoList {store} /></div>

            <p class="hint">Open this page in a second tab: changes show up in both, pushed over SSE.</p>
        </Layout>
    }
    .await
}

// `html_async!` is an `async move` block, so the state is borrowed out of the
// `Arc` first and the block captures the reference.
async fn index(State(state): State<Shared>) -> Markup {
    let state = &*state;
    html_async! { <Page {state} /> }.await
}

#[derive(Deserialize)]
struct NewTodo {
    title: String,
}

// `send` fails only when no tab is connected, so its result is ignored.
async fn add_todo(State(state): State<Shared>, Form(form): Form<NewTodo>) -> StatusCode {
    let title = form.title.trim();
    if title.is_empty() {
        return StatusCode::UNPROCESSABLE_ENTITY;
    }
    state.store.add(title.to_owned()).await;
    let _ = state.updates.send(Update::Todos);
    StatusCode::NO_CONTENT
}

async fn toggle_todo(State(state): State<Shared>, Path(id): Path<u32>) -> StatusCode {
    if !state.store.toggle(id).await {
        return StatusCode::NOT_FOUND;
    }
    let _ = state.updates.send(Update::Todos);
    StatusCode::NO_CONTENT
}

async fn delete_todo(State(state): State<Shared>, Path(id): Path<u32>) -> StatusCode {
    if !state.store.remove(id).await {
        return StatusCode::NOT_FOUND;
    }
    let _ = state.updates.send(Update::Todos);
    StatusCode::NO_CONTENT
}

/// Counts an SSE connection for as long as it is alive.
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

/// The event name matches the `sse-swap` of the element it replaces.
async fn render_update(state: &AppState, update: Update) -> Event {
    let (name, markup) = match update {
        Update::Todos => {
            let store = &state.store;
            ("todos", html_async! { <TodoList {store} /> }.await)
        }
        Update::Viewers => {
            let count = state.viewers.load(Ordering::Relaxed);
            ("viewers", html! { <Viewers {count} /> })
        }
        Update::Clock => ("clock", html! { <Clock /> }),
    };
    Event::default().event(name).data(markup.into_string())
}

async fn events(State(state): State<Shared>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Subscribed before the guard is created, so this tab also receives its
    // own viewer count.
    let updates = BroadcastStream::new(state.updates.subscribe())
        // A tab that fell behind gets a fresh list.
        .map(|msg| msg.unwrap_or(Update::Todos));
    let guard = ViewerGuard::new(state.clone());

    let ticks =
        IntervalStream::new(tokio::time::interval(Duration::from_secs(1))).map(|_| Update::Clock);

    let stream = stream::select(updates, ticks).then(move |update| {
        let state = state.clone();
        async move { render_update(&state, update).await }
    });

    // Moving the guard into the stream ties it to the connection.
    let stream = stream.map(move |event| {
        let _keep_alive = &guard;
        Ok(event)
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn not_found() -> impl IntoResponse {
    (StatusCode::NOT_FOUND, html! { <h1>Not found</h1> })
}

#[tokio::main]
async fn main() {
    // A tab that falls more than 64 updates behind misses some. Each SSE
    // connection subscribes its own receiver.
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

    let addr = std::env::var("ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    println!("listening on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await
        .expect("server");
}
