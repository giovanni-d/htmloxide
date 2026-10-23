# htmloxide

Write HTML in Rust that looks like HTML.

```rust
use htmloxide::{component, html, Markup};

#[component]
fn Greeting(name: &str, children: Markup) -> Markup {
    html! {
        <section class="greeting">
            <h1>Hello, {name}!</h1>
            {children}
        </section>
    }
}

let page = html! {
    <Greeting name="Ferris">
        <p>Welcome aboard.</p>
    </Greeting>
};
```

- **Real HTML syntax**: tags, attributes, `hx-*`, `data-*`, `aria-*`, `<!DOCTYPE html>`
- **Safe by default**: every `{value}` is HTML-escaped
- **Components**: plain functions; props are checked at compile time
- **Async components**: fetch data where it's rendered
- **Fast**: templates compile to straight `String` writes; there's no runtime parsing
- **Made for server-rendered apps**: pairs well with axum + htmx

## Getting started

```sh
cargo add --git https://github.com/giovanni-d/htmloxide htmloxide
```

## The basics

```rust
let user = User { name: "Ada".into(), admin: true };
let items = ["one", "two"];

html! {
    <p class="hello">Hi {user.name}, you have {items.len()} items.</p>

    // attributes: static, dynamic, boolean, optional
    <input type="checkbox" checked={user.admin} title={None::<&str>}>
    <button hx-post="/save" hx-swap="none">"Save"</button>

    // lists and conditionals are just Rust
    <ul>{items.iter().map(|i| html! { <li>{i}</li> }).collect::<Vec<_>>()}</ul>
    {user.admin.then(|| html! { <a href="/admin">Admin</a> })}
}
```

`each(iter)` renders a lazy iterator without collecting it first.

## Components

Any function marked `#[component]` can be used as a tag. Its arguments become props:

```rust
#[component]
fn Card(
    title: &str,                                      // required
    #[prop(optional, into)] subtitle: Option<String>, // may be left out
    #[prop(default)] featured: bool,                  // defaults to false
    children: Markup,                                 // what's between the tags
) -> Markup {
    html! {
        <article data-featured={featured}>
            <h2>{title}</h2>
            {subtitle}
            {children}
        </article>
    }
}

html! {
    <Card title="Hello" subtitle="intro" featured>
        <p>Body</p>
    </Card>
}
```

| Prop attribute            | Meaning                                       |
|---------------------------|-----------------------------------------------|
| _(none)_                  | required                                      |
| `#[prop(default)]`        | optional, `Default::default()` when left out  |
| `#[prop(default = expr)]` | optional, `expr` when left out                |
| `#[prop(optional)]`       | for `Option<T>`: pass `T`, `None` when left out |
| `#[prop(into)]`           | accepts anything `Into<T>`, e.g. `"text"` for a `String` |

Mistakes are compile errors that point at the tag:

```text
error[E0277]: missing required prop `title` on `<Card>`
   |     html! { <Card><p>x</p></Card> }
   |              ^^^^ add `title={..}` to this tag
```

## Async components

```rust
#[component]
async fn UserRow(db: &Db, id: u32) -> Markup {
    let user = db.user(id).await;
    html! { <li>{user.name}</li> }
}

let page = html_async! {
    <ul>{each_async(ids.iter().map(|&id| html_async! { <UserRow db={db} id={id} /> }))}</ul>
}
.await;
```

`html_async!` awaits async components for you. `each_async` loads the items concurrently and
keeps their order.

## With htmx

`examples/live-todos` is a shared todo list built with axum, htmx and Server-Sent Events.
Open it in two tabs and every change shows up in both. Forms post to the server, and the server
pushes re-rendered components back over SSE.

```sh
cargo run -p live-todos   # http://127.0.0.1:3000
```

## How it works

`html!` is a procedural macro. It parses the markup at compile time (using
[rstml](https://crates.io/crates/rstml)) and generates code that writes directly into a `String`.
Static HTML is escaped and merged at compile time, so only your `{values}` do work at runtime.

The rendering types underneath (`Markup`, `Render`, `PreEscaped` and the escaping rules) come from
[maud](https://maud.lambda.xyz). That means anything that renders in maud renders here too, and
`Markup` from either can be mixed freely.

[ARCHITECTURE.md](ARCHITECTURE.md) has the details.

## Limitations

- Text has to be valid Rust tokens. Quote anything that isn't: `<p>"don't"</p>`.
- Component props must be Rust identifiers, so `data-x` works on HTML tags but not on components.
- There's no `@if` / `@for` syntax inside the markup. Use normal Rust expressions in `{..}`.
