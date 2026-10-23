//! Renders a small todo page with htmloxide and prints the HTML.
//!
//! Run it with `cargo run -p basic`. It shows components with required,
//! default and `into` props, children, attributes that come and go with a
//! value, HTML escaping, and loops and conditionals written as plain Rust.

use htmloxide::{Markup, component, each, html};

// --------------------------------------------------
// Data: a todo
// --------------------------------------------------

struct Todo {
    id: u32,
    title: String,
    done: bool,
}

// --------------------------------------------------
// Component: Layout
// --------------------------------------------------

// The page shell. `title` is `into`, so the tag can pass a `&str` literal;
// `children` is the page content between `<Layout>` and `</Layout>`.
#[component]
fn Layout(#[prop(into)] title: String, children: Markup) -> Markup {
    html! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <title>{&title}</title>
                // CSS in a string literal is written out unescaped.
                <style>"body { font-family: sans-serif; } .done { opacity: .5; }"</style>
            </head>
            <body>
                <h1>{&title}</h1>
                {children}
            </body>
        </html>
    }
}

// --------------------------------------------------
// Component: TodoItem
// --------------------------------------------------

// One list item. `class` is an `Option`: `Some("done")` writes the attribute
// and `None` leaves it out. `checked` is a `bool`: present when true. The
// `hx-post` value is built at runtime with `format!`.
#[component]
fn TodoItem(todo: &Todo) -> Markup {
    html! {
        <li class={todo.done.then_some("done")} data-id={todo.id}>
            <input type="checkbox" checked={todo.done} hx-post={format!("/todos/{}/toggle", todo.id)}>
            {&todo.title}
        </li>
    }
}

// --------------------------------------------------
// Component: Notice
// --------------------------------------------------

// A message box. `kind` falls back to `"info"` when the tag leaves it out.
#[component]
fn Notice(#[prop(default = "info")] kind: &str, children: Markup) -> Markup {
    html! { <p class={format!("notice notice-{kind}")}>{children}</p> }
}

// --------------------------------------------------
// Entry point: main
// --------------------------------------------------

fn main() {
    // The second title contains `<` and `&`, which come out escaped.
    let todos = [
        Todo {
            id: 1,
            title: "Write the macro".into(),
            done: true,
        },
        Todo {
            id: 2,
            title: "Escape <user> input & stuff".into(),
            done: false,
        },
    ];
    let remaining = todos.iter().filter(|t| !t.done).count();

    // `each` renders the lazy `map` iterator; `then` gives an `Option`, so
    // the success notice only appears when nothing is left.
    let page = html! {
        <Layout title="Todos">
            <Notice>You have {remaining} thing(s) left to do.</Notice>
            <ul>{each(todos.iter().map(|todo| html! { <TodoItem todo={todo} /> }))}</ul>
            {(remaining == 0).then(|| html! { <Notice kind="success">"All done!"</Notice> })}
        </Layout>
    };

    println!("{}", page.into_string());
}
