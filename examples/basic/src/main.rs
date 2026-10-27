//! Renders a small todo page and prints the HTML. Run with
//! `cargo run -p basic`.

use htmloxide::{Markup, component, html};

struct Todo {
    id: u32,
    title: String,
    done: bool,
}

#[component]
fn Layout(#[prop(into)] title: String, children: Markup) -> Markup {
    html! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <title>{title}</title>
                <style>"body { font-family: sans-serif; } .done { opacity: .5; }"</style>
            </head>
            <body>
                <h1>{title}</h1>
                {children}
            </body>
        </html>
    }
}

// `class` is an `Option`: `None` omits the attribute. `checked` is a `bool`:
// the attribute is present only when it is `true`.
#[component]
fn TodoItem(todo: &Todo) -> Markup {
    html! {
        <li class={todo.done.then_some("done")} data-id={todo.id}>
            <input type="checkbox" checked={todo.done} hx-post={format!("/todos/{}/toggle", todo.id)}>
            {todo.title}
        </li>
    }
}

#[component]
fn Notice(#[prop(default = "info")] kind: &str, children: Markup) -> Markup {
    html! { <p class={format!("notice notice-{kind}")}>{children}</p> }
}

fn main() {
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

    let page = html! {
        <Layout title="Todos">
            <Notice>You have {remaining} thing(s) left to do.</Notice>
            <ul>@for todo in &todos { <TodoItem {todo} /> }</ul>
            @if remaining == 0 { <Notice kind="success">All done!</Notice> }
        </Layout>
    };

    println!("{}", page.into_string());
}
