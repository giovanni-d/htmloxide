//! `html_async!` and async components: awaiting, nesting, futures inside
//! `{..}`, concurrency with `each_async`, and borrowing.

use std::time::{Duration, Instant};

use htmloxide::{Markup, component, each_async, html, html_async};
use tokio::time::sleep;

// Each lookup takes 50ms, so that sequential and concurrent lookups take
// measurably different times.
struct Db;
impl Db {
    async fn user_name(&self, id: u32) -> String {
        sleep(Duration::from_millis(50)).await;
        format!("user-{id}")
    }
}

#[component]
async fn UserName(db: &Db, id: u32) -> Markup {
    let name = db.user_name(id).await;
    html! { <b>{name}</b> }
}

#[component]
async fn Panel(#[prop(into)] title: String, children: Markup) -> Markup {
    sleep(Duration::from_millis(1)).await;
    html! { <section><h2>{title}</h2>{children}</section> }
}

#[component]
fn Sync(label: &str) -> Markup {
    html! { <i>{label}</i> }
}

#[component]
fn Titled(title: Markup, children: Markup) -> Markup {
    html! { <section><h2>{title}</h2>{children}</section> }
}

// `html_async!` is an `async move` block: borrowing `db` first makes the
// future hold a `&Db` instead of taking the `Db`.
#[tokio::test]
async fn async_component_is_awaited() {
    let db = Db;
    let db = &db;
    let out = html_async! { <p>Hello <UserName db={db} id=7 />!</p> }.await;
    assert_eq!(out.into_string(), "<p>Hello <b>user-7</b>!</p>");
}

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

#[tokio::test]
async fn futures_in_blocks_are_awaited() {
    let db = &Db;
    let out = html_async! { <p>{db.user_name(3)} / {db.user_name(4).await}</p> }.await;
    assert_eq!(out.into_string(), "<p>user-3 / user-4</p>");
}

#[tokio::test]
async fn async_components_in_control_flow() {
    let db = &Db;
    let ids = [1, 2];
    let out = html_async! {
        <ul>
            @for id in ids {
                @if id > 1 { <li><UserName db={db} id={id} /></li> }
            }
        </ul>
    }
    .await;
    assert_eq!(out.into_string(), "<ul><li><b>user-2</b></li></ul>");
}

#[tokio::test]
async fn async_components_in_markup_props() {
    let db = &Db;
    let out = html_async! { <Titled title={<UserName db={db} id=5 />}><p>x</p></Titled> }.await;
    assert_eq!(
        out.into_string(),
        "<section><h2><b>user-5</b></h2><p>x</p></section>"
    );
}

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
    // Sequential lookups would take 500ms.
    assert!(elapsed < Duration::from_millis(250), "took {elapsed:?}");
}

#[tokio::test]
async fn place_expressions_are_borrowed_not_moved() {
    let name = String::from("Ada");
    let n = &name;
    let out = html_async! { <p>{n}</p> }.await;
    assert_eq!(name, "Ada");
    assert_eq!(out.into_string(), "<p>Ada</p>");
}

#[tokio::test]
async fn returned_from_functions() {
    #[allow(clippy::manual_async_fn)] // the point: html_async! is a plain future
    fn page(db: &Db) -> impl Future<Output = Markup> + '_ {
        html_async! { <main><UserName db={db} id=2 /></main> }
    }
    assert_eq!(page(&Db).await.into_string(), "<main><b>user-2</b></main>");
}
