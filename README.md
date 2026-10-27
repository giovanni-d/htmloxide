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
- **Safe by default**: every `{value}` is escaped for the place it's written in, so user input
  can't add tags, attributes or scripts
- **Components**: plain functions, with props checked at compile time
- **Async components**: fetch data where it's rendered
- **Fast**: templates compile to `String` writes, with no parsing at runtime
- **Made for server-rendered apps**: pairs well with axum and htmx

## Getting started

```sh
cargo add --git https://github.com/giovanni-d/htmloxide htmloxide
```

## The basics

`html!` takes HTML with Rust expressions in `{..}` and evaluates to `Markup`: a `String` of HTML
marked as already escaped, so `{markup}` inside another `html!` is written as it is.
`.into_string()` returns the `String`.

```rust
use htmloxide::html;

let name = "Ada";
let unread = 3;

let page = html! {
    <p class="greeting">Hello, {name}! You have {unread} new messages.</p>
    <button disabled={unread == 0}>Mark all as read</button>
};

let body: String = page.into_string();
```

`body` is:

```html
<p class="greeting">Hello, Ada! You have 3 new messages.</p><button>Mark all as read</button>
```

`disabled` is left out because `unread == 0` is `false`.

### Values

Between tags, `{expr}` writes the value of `expr` according to its type:

| Type                                                   | Written as                                  |
|--------------------------------------------------------|---------------------------------------------|
| `Markup`                                               | HTML, unchanged                             |
| `&str`, `String`, numbers                              | escaped text                                |
| other types implementing `Render`                      | what their `Render` impl writes             |
| `Vec`, slices, arrays and `Option` of the types above  | each item in order; `None` writes nothing   |
| other types implementing `Display`                     | escaped text                                |

A block borrows its value, so `{user.name}` leaves `user` usable afterwards. A lazy iterator,
such as `items.iter().map(..)`, is consumed as it runs and can't be written from a borrow. Write
the loop with `@for`, or wrap the iterator in `each`:

```rust
use htmloxide::{each, html};

let items = ["one", "two"];

html! {
    <ul>@for item in &items { <li>{item}</li> }</ul>
    <ul>{each(items.iter().map(|item| html! { <li>{item}</li> }))}</ul>
}
```

Both lists are `<ul><li>one</li><li>two</li></ul>`. `@for` loops in place and writes into the same
buffer as the markup around it ([Control flow](#control-flow)). `each` is for an iterator that
already exists, such as one returned by a function.

### Attributes

An attribute is written as `name="text"`, `name={expr}` or `name` alone. `{name}` is shorthand for
`name={name}`; for a name that is a Rust keyword, `{r#type}` is shorthand for `type={r#type}`.

`name={expr}` writes the value of `expr`, escaped. For a `bool` or `Option<bool>`, the output
depends on the attribute:

| Attribute                                                                         | `true`        | `false`        | `None`   |
|-----------------------------------------------------------------------------------|---------------|----------------|----------|
| `aria-*`, `data-*`, `draggable`, `spellcheck`, `contenteditable`                  | `name="true"` | `name="false"` | left out |
| `hx-boost`, `hx-validate`, `hx-push-url`, `hx-replace-url`, `hx-history`, `hx-swap-oob`, and their `data-hx-*` forms | `name="true"` | `name="false"` | left out |
| any other, such as `disabled`, `checked`, `hx-preserve` or `data-hx-preserve`     | `name`        | left out       | left out |

For any other `Option`, `Some(value)` writes the attribute and `None` leaves it out.

```rust
let done = false;
let note: Option<&str> = None;

html! { <input type="checkbox" checked={done} aria-checked={done} title={note}> }
```

writes `<input type="checkbox" aria-checked="false">`.

Attributes that run code, load URLs or hold HTML, such as `onclick`, `href` and `srcdoc`,
[take values by stricter rules](#code-urls-and-html-in-attributes).

### Text

Unquoted text is read as Rust tokens. Quoted text is written exactly as it is, spaces and line
breaks included. Quotes are required for:

- An unmatched `'`, which is not a valid Rust token: `<p>"don't"</p>`.
- A `{..}` inside `(..)` or `[..]`: the brackets are read as one token, so the block inside them is
  a compile error. Quote the brackets: `<p>"("{count}")"</p>`.
- `@` followed by `if`, `for`, `match`, `let` or `else`, which starts control flow:
  `<p>"@if"</p>`.

### Whitespace

Unquoted text and the space between tags follow the whitespace rules of JSX, so markup can be
indented freely:

- Text over several lines is trimmed line by line and joined with single spaces.
- Between tags, `{..}` blocks and control flow, spaces within one line become one space, and a
  gap containing a line break becomes nothing. A `" "` writes a space across a line break.

```rust
html! {
    <p>
        Text over
        two lines
    </p>
    <b>a</b> <i>b</i>
    <i>c</i>
    " "
    <i>d</i>
}
```

writes `<p>Text over two lines</p><b>a</b> <i>b</i><i>c</i> <i>d</i>`.

### Tags

- `<!DOCTYPE html>` is written as it is.
- Void elements, such as `<br>`, `<img>` and `<input>`, have no closing tag and need no `/>`.
- Any other self-closed tag gets a closing tag: `<div />` writes `<div></div>`.
- HTML comments (`<!-- -->`) are a compile error. Rust comments (`// ..`) are allowed and don't
  appear in the output HTML.

## Control flow

`@if`, `@for`, `@match` and `@let` work like their Rust counterparts, with markup in the braces:

```rust
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
```

- Only the opening keyword takes `@`; `else` and `else if` are written as in Rust.
- Every body is markup in braces, `@match` arms included.
- The spaces just inside a body's braces are formatting: `@if done { yes }` writes `yes`.
- `@let` binds a value for the rest of the body it's in.
- Each construct compiles to a plain `if`, `for` or `match` that writes into the same buffer, with
  no closures and no extra allocations.

## Components

A function marked `#[component]` can be used as a tag. Its arguments become the tag's attributes,
called props, and the markup between the opening and closing tags is passed as its `children`
argument. Components are named in UpperCamelCase: a tag that starts with an uppercase letter is a
component, and a lowercase one is HTML.

```rust
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

html! {
    <Card title="Hello" subtitle="intro" featured>
        <p>Body</p>
    </Card>
}
```

Props are written like attributes, `{title}` shorthand included. A prop written without a value,
like `featured`, is `true`. How a prop is declared decides what the tag can pass, and what the
prop is when the tag leaves it out:

| Declaration                    | When left out   | Accepts                                  |
|--------------------------------|-----------------|------------------------------------------|
| `x: T`                         | compile error   | `T`                                      |
| `x: Option<T>`                 | `None`          | `T`, `Option<T>`                         |
| `#[prop(default)] x: T`        | `T::default()`  | `T`                                      |
| `#[prop(default = expr)] x: T` | `expr`          | `T`                                      |
| `#[prop(into)] x: T`           | compile error   | `impl Into<T>`                           |
| `#[prop(into)] x: Option<T>`   | `None`          | `impl Into<T>`, `Option<impl Into<T>>`   |

`default` and `into` can be combined. On an `Option<T>` prop, `default` replaces `None` as the
value used when the prop is left out. `children` follows the same rules, so
`children: Option<Markup>` makes the content optional.

A prop named like a Rust keyword is declared as a raw identifier and written without the `r#`:
`r#type: &str` is set with `type="text"`. A component in another module is used by its path,
`<ui::Card>`, with nothing else to import.

A prop of type `Markup` can take markup in braces:

```rust
#[component]
fn Dialog(title: Markup, children: Markup) -> Markup {
    html! { <dialog><h2>{title}</h2>{children}</dialog> }
}

html! {
    <Dialog title={<b>Delete the file?</b>}>
        <p>This cannot be undone.</p>
    </Dialog>
}
```

The markup must start with one of:

- a tag: `{<b>Hi</b>}`
- a fragment, `<>..</>`, for markup that starts with text: `{<>Hi <b>there</b></>}`
- control flow, `@if`, `@for`, `@match` or `@let`: `{@if admin { <b>Admin</b> } else { User }}`

Only component props take markup this way; HTML attribute values are text.

Mistakes are compile errors that point at the tag:

```text
error[E0277]: missing required prop `title` on `<Card>`
   |     html! { <Card><p>x</p></Card> }
   |              ^^^^ add `title={..}` to this tag
```

## Async components

A component can be an `async fn`. Markup that uses one is built with `html_async!` instead of
`html!`:

```rust
#[component]
async fn UserRow(db: &Db, id: u32) -> Markup {
    let user = db.user(id).await;
    html! { <li>{user.name}</li> }
}

let page = html_async! {
    <ul>{each_async(ids.iter().map(|&id| html_async! { <UserRow {db} {id} /> }))}</ul>
}
.await;
```

`html_async!` evaluates to a future that awaits each async component, and each future written in a
`{..}` block, in document order. Sync components work inside it too. An async component inside
`html!` is a compile error that points to `html_async!`.

The future is an `async move` block: it takes ownership of the variables it uses. To keep using a
value afterwards, borrow it before the macro: `let db = &db;`.

`each_async` awaits its items concurrently and keeps their order; in `@for`, they're awaited one
after another.

## With axum and htmx

```sh
cargo add --git https://github.com/giovanni-d/htmloxide htmloxide --features axum
```

With the `axum` feature, `Markup` implements axum's `IntoResponse`, so a handler can return it
directly: `async fn index() -> Markup { html! { .. } }`.

`examples/live-todos` is a shared todo list built with axum, htmx and Server-Sent Events.
Open it in two tabs and every change shows up in both. Forms post to the server, and the server
pushes re-rendered components back over SSE.

```sh
cargo run -p live-todos   # http://127.0.0.1:3000
```

htmx attributes are ordinary attributes:

```rust
html! { <button hx-post={format!("/todos/{id}/toggle")} hx-target="#todos">Toggle</button> }
```

## JavaScript

JavaScript code is a `Js` value, built only from the source: by a `js!` template, or by
`Js::from_static` for code compiled into the program. Code written literally in the markup, such
as `onclick="save()"` or `<script>"init();"</script>`, is code too. A value computed at runtime is
data, never code.

`js!` works like `format!`: the template is code, and each `{..}` is a value written as JSON, so
a string stays a string whatever it contains. Values can also be passed as arguments:
`js!("go({})", id + 1)`.

```rust
let id = 7;
let name = "Ada";
let todos = ["one", "two"];

html! {
    <button onclick={js!("select({id}, {name})")}>Select</button>
    <script>{js!("const user = {name};")}</script>
    <script type="application/json" id="todos">{todos}</script>
    <script>{Js::from_static(APP_JS)}</script>
}
```

Inside `<script>`:

- A string literal is code: `<script>"init();"</script>`.
- A `{value}` is JSON data. Any `serde::Serialize` value can be data, including your own structs
  with `#[derive(Serialize)]`.
- A `Js` value is code: a `js!` template, or `Js::from_static(include_str!("app.js"))` for a file
  compiled into the program.
- Code with values in it is written as one `{js!(..)}`. A literal next to a block, as in
  `<script>"let x = " {x}</script>`, is a compile error.

Inside `<style>`, a string literal or a `{value}` is CSS text.

## Code, URLs and HTML in attributes

Some attributes make the browser run code, load a URL or parse HTML. For these, `name={value}`
follows stricter rules:

| Attribute                                     | `name={value}`                                           |
|-----------------------------------------------|----------------------------------------------------------|
| `on*`, `hx-on*`, `hx-vars`                    | must be `Js`                                             |
| `href`, `src`, `action`, `formaction`, `xlink:href`, `poster`, `cite`, `data` | a `javascript:` or `vbscript:` URL is replaced with `about:invalid#blocked`; `Js` is written as `javascript:` + code |
| `<script src>`, `<base href>`                 | must be a `&'static str`                                 |
| `hx-vals`, `hx-headers`, `hx-request`         | a string as written; maps, `Vec`s and `Serialize` types as JSON; `Js` as `js:` + code |
| `srcdoc`                                      | a string is text in the document; `Markup` is HTML       |
| any other                                     | escaped text; `Markup` is escaped, so the attribute's value is its HTML |

Fixed values, such as `onclick="save()"`, are written as given.

## Security

A value computed at runtime is written as HTML only if it is `Markup`, and as JavaScript only if it
is `Js`. Any other value is written as text or data, escaped for where it appears.

### Text and data

A string, a number, or a `Display` or `Serialize` value is written as:

| Where                                                                         | Written as                                                                                  |
|-------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------|
| Between tags                                                                  | text, with `&`, `<`, `>` and `"` replaced by `&amp;`, `&lt;`, `&gt;` and `&quot;`           |
| Attribute value                                                               | text escaped the same way, inside double quotes                                             |
| `on*`, `hx-on*`, `hx-vars`                                                    | a compile error: only `Js` is accepted                                                      |
| `href`, `src`, `action`, `formaction`, `xlink:href`, `poster`, `cite`, `data` | escaped text; a `javascript:` or `vbscript:` URL is replaced by `about:invalid#blocked`     |
| `<script src>`, `<base href>`                                                 | a compile error: only a `&'static str` is accepted                                          |
| `srcdoc`                                                                      | text escaped twice, so the framed document shows it as text                                 |
| Inside `<script>`, and `{..}` in `js!`                                        | JSON, with `<`, `>`, `&`, U+2028 and U+2029 written as `\u` escapes                         |
| Inside `<style>`                                                              | the string, with each `</style` rewritten so it can't end the element                       |

A URL's scheme is read as a browser reads it: in any letter case, after leading spaces and control
characters, with tabs and line breaks ignored.

### `Markup` and `Js`

`Markup` is written unchanged between tags, and as escaped text in an attribute. It comes from
`html!`, which escapes every value by the rules above, or from `PreEscaped`, which writes its
content unchanged. The output of a custom `Render` impl is also written unchanged between tags; in
an attribute, only its `"` is escaped, so it can't end the attribute.

`Js` comes from `js!`, which writes every value as JSON, or from `Js::from_static`, which takes
only a `&'static str`. Inside `<script>`, each `<script` and `</script` in a `Js` is rewritten so it
can't end the element.

### Names and fixed text

- Tag and attribute names come only from the source. On an HTML tag, `{name}` is shorthand for
  `name={name}` and must be a single name.
- Attribute names are matched in any letter case: `ONCLICK` also accepts only `Js`, and `HREF` is
  checked as a URL. `<script>` and `<style>` must be written in lowercase.
- Fixed text in the source, such as `onclick="save()"` or `href="javascript:history.back()"`, is
  written as given. Fixed code that would end its `<script>` or `<style>` element is a compile
  error.

### Not covered

- **`PreEscaped` and custom `Render` impls** write HTML without escaping. User input passed
  through either can add elements, attributes and scripts.
- **htmx expressions.** htmx runs a value that starts with `js:` or `javascript:` in `hx-vals`,
  `hx-headers` and `hx-request`, and a `[..]` filter in `hx-trigger`, as JavaScript. Strings in
  these attributes are escaped but not checked; user input must not reach them.
- **htmx requests.** The URLs in `hx-get`, `hx-post`, `hx-put`, `hx-patch` and `hx-delete` are
  escaped but not checked, and htmx inserts the response into the page. htmx 2 sends requests
  only to the page's own origin unless `htmx.config.selfRequestsOnly` is `false`.
- **Other frameworks.** Attributes that other libraries run as JavaScript, such as Alpine's `x-*`
  and Vue's `v-*`, are escaped text.
- **CSS.** CSS from user input, in `style` or `<style>`, can restyle the page and send attribute
  values to another server through attribute selectors and `url()`. Current browsers don't run
  JavaScript from CSS.
- **Other URLs.** Only `javascript:` and `vbscript:` URLs are blocked. Any other URL, such as a
  `data:` URL or one to another site, is written as given.
- **`Box::leak`.** A `&'static str` can be made from runtime data with `Box::leak`, and is then
  accepted by `<script src>`, `<base href>` and `Js::from_static`.

## How it works

`html!` is a procedural macro. It parses the markup at compile time (using
[rstml](https://crates.io/crates/rstml)) and generates code that writes directly into a `String`.
Static HTML is escaped and merged at compile time, so only your `{values}` do work at runtime.

The rendering types underneath (`Markup`, `Render`, `PreEscaped` and the escaping rules) come from
[maud](https://maud.lambda.xyz). Anything that renders in maud renders here too, and `Markup` from
either can be mixed freely.

[INTERNALS.md](INTERNALS.md) has the details.

## Limitations

- Component props must be Rust identifiers, so `data-x` works on HTML tags but not on components.
- Tag names are fixed at compile time: `<{tag}>` is not supported.
- A prop can't have an `impl Trait` type; use a generic parameter instead.
