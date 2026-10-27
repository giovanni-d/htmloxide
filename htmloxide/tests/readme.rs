//! Each test mirrors a snippet of README.md, so a README example that stops
//! compiling or changes its output fails here. A change to a snippet needs
//! the same change to its test.

use htmloxide::{Js, Markup, component, each, each_async, html, html_async, js};

#[component]
fn Greeting(name: &str, children: Markup) -> Markup {
    html! {
        <section class="greeting">
            <h1>Hello, {name}!</h1>
            {children}
        </section>
    }
}

#[test]
fn intro() {
    let page = html! {
        <Greeting name="Ferris">
            <p>Welcome aboard.</p>
        </Greeting>
    };
    assert_eq!(
        page.into_string(),
        r#"<section class="greeting"><h1>Hello, Ferris!</h1><p>Welcome aboard.</p></section>"#
    );
}

#[test]
fn basics() {
    let name = "Ada";
    let unread = 3;

    let page = html! {
        <p class="greeting">Hello, {name}! You have {unread} new messages.</p>
        <button disabled={unread == 0}>Mark all as read</button>
    };

    let body: String = page.into_string();
    assert_eq!(
        body,
        r#"<p class="greeting">Hello, Ada! You have 3 new messages.</p><button>Mark all as read</button>"#
    );
}

#[test]
fn basics_iterators() {
    let items = ["one", "two"];

    let out = html! {
        <ul>@for item in &items { <li>{item}</li> }</ul>
        <ul>{each(items.iter().map(|item| html! { <li>{item}</li> }))}</ul>
    };
    assert_eq!(
        out.into_string(),
        "<ul><li>one</li><li>two</li></ul><ul><li>one</li><li>two</li></ul>"
    );
}

#[test]
fn basics_attributes() {
    let done = false;
    let note: Option<&str> = None;

    let out = html! { <input type="checkbox" checked={done} aria-checked={done} title={note}> };
    assert_eq!(
        out.into_string(),
        r#"<input type="checkbox" aria-checked="false">"#
    );
}

#[test]
fn basics_whitespace() {
    let out = html! {
        <p>
            Text over
            two lines
        </p>
        <b>a</b> <i>b</i>
        <i>c</i>
        " "
        <i>d</i>
    };
    assert_eq!(
        out.into_string(),
        "<p>Text over two lines</p><b>a</b> <i>b</i><i>c</i> <i>d</i>"
    );
}

struct User {
    name: String,
}

struct Todo {
    title: &'static str,
    done: bool,
}

#[test]
fn control_flow() {
    let render = |user: Option<User>, todos: &[Todo]| {
        html! {
            @if let Some(user) = &user {
                <p>Signed in as {user.name}.</p>
            } else {
                <a href="/login">Log in</a>
            }

            <ul>
                @for (i, todo) in todos.iter().enumerate() {
                    @let number = i + 1;
                    <li class={todo.done.then_some("done")}>{number}. {todo.title}</li>
                }
            </ul>

            @match todos.len() {
                0 => { <p>Nothing to do.</p> }
                1 => { <p>One thing left.</p> }
                n => { <p>{n} things left.</p> }
            }
        }
        .into_string()
    };
    let ada = User { name: "Ada".into() };
    let todos = [
        Todo {
            title: "Write",
            done: true,
        },
        Todo {
            title: "Ship",
            done: false,
        },
    ];
    assert_eq!(
        render(Some(ada), &todos),
        concat!(
            "<p>Signed in as Ada.</p>",
            r#"<ul><li class="done">1. Write</li><li>2. Ship</li></ul>"#,
            "<p>2 things left.</p>",
        )
    );
    assert_eq!(
        render(None, &[]),
        r#"<a href="/login">Log in</a><ul></ul><p>Nothing to do.</p>"#
    );
}

#[component]
fn Card(
    title: &str,
    #[prop(into)] subtitle: Option<String>,
    #[prop(default)] featured: bool,
    children: Markup,
) -> Markup {
    html! {
        <article data-featured={featured}>
            <h2>{title}</h2>
            {subtitle}
            {children}
        </article>
    }
}

#[test]
fn components() {
    let out = html! {
        <Card title="Hello" subtitle="intro" featured>
            <p>Body</p>
        </Card>
    };
    assert_eq!(
        out.into_string(),
        r#"<article data-featured="true"><h2>Hello</h2>intro<p>Body</p></article>"#
    );
}

#[component]
fn Dialog(title: Markup, children: Markup) -> Markup {
    html! { <dialog><h2>{title}</h2>{children}</dialog> }
}

#[test]
fn markup_props() {
    let out = html! {
        <Dialog title={<b>Delete the file?</b>}>
            <p>This cannot be undone.</p>
        </Dialog>
    };
    assert_eq!(
        out.into_string(),
        "<dialog><h2><b>Delete the file?</b></h2><p>This cannot be undone.</p></dialog>"
    );
}

// The README leaves `Db` undefined; this is the least that makes the snippet
// compile.
struct Db;
struct DbUser {
    name: String,
}
impl Db {
    async fn user(&self, id: u32) -> DbUser {
        DbUser {
            name: format!("user {id}"),
        }
    }
}

#[component]
async fn UserRow(db: &Db, id: u32) -> Markup {
    let user = db.user(id).await;
    html! { <li>{user.name}</li> }
}

#[tokio::test]
async fn async_components() {
    let db = &Db;
    let ids = [1u32, 2];
    let page = html_async! {
        <ul>{each_async(ids.iter().map(|&id| html_async! { <UserRow {db} {id} /> }))}</ul>
    }
    .await;
    assert_eq!(
        page.into_string(),
        "<ul><li>user 1</li><li>user 2</li></ul>"
    );
}

#[test]
fn with_htmx() {
    let id = 7;
    let out = html! { <button hx-post={format!("/todos/{id}/toggle")} hx-target="#todos">Toggle</button> };
    assert_eq!(
        out.into_string(),
        r##"<button hx-post="/todos/7/toggle" hx-target="#todos">Toggle</button>"##
    );
}

#[test]
fn javascript() {
    const APP_JS: &str = "boot();";
    let id = 7;
    let name = "Ada";
    let todos = ["one", "two"];
    let out = html! {
        <button onclick={js!("select({id}, {name})")}>Select</button>
        <script>{js!("const user = {name};")}</script>
        <script type="application/json" id="todos">{todos}</script>
        <script>{Js::from_static(APP_JS)}</script>
    };
    assert_eq!(
        out.into_string(),
        concat!(
            r#"<button onclick="select(7, &quot;Ada&quot;)">Select</button>"#,
            r#"<script>const user = "Ada";</script>"#,
            r#"<script type="application/json" id="todos">["one","two"]</script>"#,
            "<script>boot();</script>",
        )
    );
}
