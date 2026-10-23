//! Each test mirrors a snippet from README.md, so a README example that stops
//! compiling or changes its output fails here. When you change a snippet in
//! the README, change its test to match.

use htmloxide::{Markup, component, each_async, html, html_async};

// --------------------------------------------------
// README: intro
// --------------------------------------------------

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

// --------------------------------------------------
// README: the basics
// --------------------------------------------------

struct User {
    name: String,
    admin: bool,
}

#[test]
fn basics() {
    let user = User {
        name: "Ada".into(),
        admin: true,
    };
    let items = ["one", "two"];

    let out = html! {
        <p class="hello">Hi {user.name}, you have {items.len()} items.</p>

        // attributes: static, dynamic, boolean, optional
        <input type="checkbox" checked={user.admin} title={None::<&str>}>
        <button hx-post="/save" hx-swap="none">"Save"</button>

        // lists and conditionals are just Rust
        <ul>{items.iter().map(|i| html! { <li>{i}</li> }).collect::<Vec<_>>()}</ul>
        {user.admin.then(|| html! { <a href="/admin">Admin</a> })}
    };
    assert_eq!(
        out.into_string(),
        concat!(
            r#"<p class="hello">Hi Ada, you have 2 items.</p>"#,
            r#"<input type="checkbox" checked>"#,
            r#"<button hx-post="/save" hx-swap="none">Save</button>"#,
            "<ul><li>one</li><li>two</li></ul>",
            r#"<a href="/admin">Admin</a>"#,
        )
    );
}

// --------------------------------------------------
// README: components
// --------------------------------------------------

#[component]
fn Card(
    title: &str,
    #[prop(optional, into)] subtitle: Option<String>,
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
        "<article data-featured><h2>Hello</h2>intro<p>Body</p></article>"
    );
}

// --------------------------------------------------
// README: async components
// --------------------------------------------------

// The README leaves `Db` undefined; this is the smallest version that makes
// the snippet compile.
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
        <ul>{each_async(ids.iter().map(|&id| html_async! { <UserRow db={db} id={id} /> }))}</ul>
    }
    .await;
    assert_eq!(
        page.into_string(),
        "<ul><li>user 1</li><li>user 2</li></ul>"
    );
}
