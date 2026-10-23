//! Tests for `html_async!` and async components: awaiting, nesting, futures
//! inside `{..}`, concurrency with `each_async`, and borrowing.

use std::time::{Duration, Instant};

use htmloxide::{Markup, component, each_async, html, html_async};
use tokio::time::sleep;

// --------------------------------------------------
// Fixtures: a fake database and three components
// --------------------------------------------------

// Every lookup takes 50ms, long enough for the concurrency test to tell
// sequential lookups from concurrent ones.
struct Db;
impl Db {
    async fn user_name(&self, id: u32) -> String {
        sleep(Duration::from_millis(50)).await;
        format!("user-{id}")
    }
}

// An async component: it awaits a lookup, then renders.
#[component]
async fn UserName(db: &Db, id: u32) -> Markup {
    let name = db.user_name(id).await;
    html! { <b>{name}</b> }
}

// An async component with children.
#[component]
async fn Panel(#[prop(into)] title: String, children: Markup) -> Markup {
    sleep(Duration::from_millis(1)).await;
    html! { <section><h2>{title}</h2>{children}</section> }
}

// A sync component, to mix with the async ones.
#[component]
fn Sync(label: &str) -> Markup {
    html! { <i>{label}</i> }
}

// --------------------------------------------------
// Tests: awaiting
// --------------------------------------------------

// `html_async!` is an `async move` block, so `db` is borrowed first: the
// future then holds a `&Db` instead of taking the `Db`.
#[tokio::test]
async fn async_component_is_awaited() {
    let db = Db;
    let db = &db;
    let out = html_async! { <p>Hello <UserName db={db} id=7 />!</p> }.await;
    assert_eq!(out.into_string(), "<p>Hello <b>user-7</b>!</p>");
}

// The children of an async component can hold both async and sync
// components.
#[tokio::test]
async fn nesting_children_and_mixing_sync_components() {
    let db = &Db;
    let out = html_async! {
        <Panel title="Users">
            <UserName db={db} id=1 />
            <Sync label="sync inside async" />
        </Panel>
    }
    .await;
    assert_eq!(
        out.into_string(),
        "<section><h2>Users</h2><b>user-1</b><i>sync inside async</i></section>"
    );
}

// `{db.user_name(3)}` is a future, awaited by the macro; `{.. .await}` is
// awaited by hand. Both render the same way.
#[tokio::test]
async fn futures_in_blocks_are_awaited() {
    let db = &Db;
    let out = html_async! { <p>{db.user_name(3)} / {db.user_name(4).await}</p> }.await;
    assert_eq!(out.into_string(), "<p>user-3 / user-4</p>");
}

// --------------------------------------------------
// Tests: concurrency
// --------------------------------------------------

// Ten 50ms lookups through `each_async` must run at the same time and still
// come out in order. The 250ms limit leaves room for a slow machine.
#[tokio::test]
async fn each_async_runs_concurrently() {
    let db = &Db;
    let ids = [1u32, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    let start = Instant::now();
    let out = html_async! {
        <ul>{each_async(ids.iter().map(|&id| html_async! { <li><UserName db={db} id={id} /></li> }))}</ul>
    }
    .await;
    let elapsed = start.elapsed();

    let expected: String = ids
        .iter()
        .map(|i| format!("<li><b>user-{i}</b></li>"))
        .collect();
    assert_eq!(out.into_string(), format!("<ul>{expected}</ul>"));
    // 10 x 50ms sequentially would be 500ms.
    assert!(elapsed < Duration::from_millis(250), "took {elapsed:?}");
}

// --------------------------------------------------
// Tests: borrowing and returning
// --------------------------------------------------

// `{n}` is a place expression, so it's rendered by reference, as in `html!`.
#[tokio::test]
async fn place_expressions_are_borrowed_not_moved() {
    let name = String::from("Ada");
    let n = &name;
    let out = html_async! { <p>{n}</p> }.await;
    assert_eq!(name, "Ada");
    assert_eq!(out.into_string(), "<p>Ada</p>");
}

// `html_async!` is an ordinary future, so a function can return it as
// `impl Future`.
#[tokio::test]
async fn returned_from_functions() {
    #[allow(clippy::manual_async_fn)] // the point: html_async! is a plain future
    fn page(db: &Db) -> impl Future<Output = Markup> + '_ {
        html_async! { <main><UserName db={db} id=2 /></main> }
    }
    assert_eq!(page(&Db).await.into_string(), "<main><b>user-2</b></main>");
}
