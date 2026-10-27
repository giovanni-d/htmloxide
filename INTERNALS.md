# Internals

## Crates

| Crate                 | Role |
|-----------------------|------|
| `htmloxide-macros`    | The procedural macros `html!`, `html_async!`, `#[component]` and `js!`. Runs only at compile time. |
| `htmloxide`           | The crate users depend on. Re-exports the macros and maud's `Markup`, `Render` and `PreEscaped`; defines `each`, `each_async` and `Js`; holds `__private`, the runtime code that the macros' output calls. |
| `examples/basic`      | Prints a page. |
| `examples/live-todos` | A shared todo list with axum, htmx and Server-Sent Events. |

A `proc-macro` crate can export nothing but macros, hence two crates. Generated code reaches the
runtime through the path `::htmloxide::__private`; `extern crate self as htmloxide` makes that
path resolve inside `htmloxide` too.

`Markup`, `Render`, `PreEscaped` and the escaping rules are [maud](https://maud.lambda.xyz)'s, so
values that render in maud render here, and `Markup` from either crate can be mixed.

## Overview

```text
html! { <a href={url}>Hi {name}</a> }
   │
   │  rstml parses the tokens (control_flow.rs adds @if/@for/@match/@let,
   │  mark_markup lets {..} hold markup)
   ▼
node tree
   │
   │  Generator walks the tree in document order
   ▼
Vec<Part>         fixed HTML, escaped now | values to write at runtime | control flow
   │
   │  Generator::finish emits one statement per part
   ▼
Rust code         buf.push_str("..") and calls into htmloxide::__private
   │
   │  compiled with the user's crate; type checking picks how each value is written
   ▼
Markup            PreEscaped<String>
```

## `html!`

All in `htmloxide-macros/src/lib.rs`, except control flow (`control_flow.rs`).

### Parse

rstml is configured in `parser_config`: void elements need no `/>`, the content of `<script>`
and `<style>` is kept as raw tokens, a `{..}` that isn't valid Rust is kept as a node
(`recover_block`, so rust-analyzer can complete inside it), `mark_markup` runs on each `{..}`, and
`ControlFlow` is the custom node type. Parsing continues past errors.

### Walk

`Generator` turns nodes into `Part`s:

| Part | Comes from | Emitted as |
|------|------------|------------|
| `Static` | tags, attribute names, text, literal attribute values | `buf.push_str("..")` |
| `Render` | `{expr}` between tags, a component tag | `render!`, or `render_async!` in `html_async!` |
| `Attr` | `key={expr}`, `{key}` | the writer for its `AttrContext` |
| `RawTextValue` | `{expr}` inside `<script>` or `<style>` | `script_value`, `style_value` |
| `Branches`, `Match`, `Let` | `@if` / `@for`, `@match`, `@let` | `if` / `for`, `match`, `let` around the statements of each body |

Everything known at compile time is decided at compile time. Text and literal attribute values are
escaped by `escape`, and consecutive `Static` parts are merged into one string. A literal `bool`
adds or drops the attribute, or writes `"true"` / `"false"` where `takes_true_false` says so.
Numbers are written in base 10.

Tag and attribute names come only from the source: `<{tag}>` and any `{..}` on a tag other than
the `{key}` shorthand are compile errors.

### Emit

```rust
html! { <a href={url} class="x">Hi {name}</a> }
```

expands to

```rust
{
    let mut buf = String::with_capacity(20);
    buf.push_str("<a");
    ::htmloxide::__private::attr!(" href", &(url), &mut buf, Url);
    buf.push_str(" class=\"x\">Hi ");
    ::htmloxide::__private::render!(&(name), &mut buf);
    buf.push_str("</a>");
    ::htmloxide::PreEscaped(buf)
}
```

- The capacity is the length of the fixed HTML outside control flow.
- `buf` is created with `Span::mixed_site()`, so user code can neither name it nor collide with it.
- Each statement carries the span of the user's code it came from (`quote_spanned!`), so a type
  error points at that code.
- Values are passed as `&(expr)`: borrowed, never moved. `{name}` is emitted as `name`, not
  `{name}` (`block_tokens`, `unbrace`), because a block expression moves its value.
- Parse and walk errors are collected and emitted as `compile_error!` next to the generated code,
  so one compilation reports every mistake and the user's expressions are still type-checked.

### Choosing a writer by type

A proc macro sees tokens, not types. The type-dependent part of writing a value happens in
`htmloxide::__private` (`htmloxide/src/lib.rs`), through autoref specialization: the macros call a
method on `&&&&Wrap(value)`, every tier implements that method for `Wrap<&T>` behind a different
number of references, and method resolution takes the tier with the most references whose bounds
hold. The comment on `__private` explains the mechanism and the rules for changing it.

- **`render!`** (between tags): `Render` → iterable of `Render` (`Vec`, slices, arrays, `Option`)
  → future (a compile error pointing to `html_async!`) → `Display`.
- **`attr!`** (attribute values): `Js` / `Option<Js>` → `Markup` / `Option<Markup>` → `bool` /
  `Option<bool>` → `Option<impl Render>` → `Render` → `Display`. A `bool` or `None` adds or drops
  the attribute; in the `Words` context, a `bool` is written as `"true"` / `"false"`. Values are
  written in double quotes; a `"` from a `Render` impl is written as `&quot;`.
- **`attr_json!`**: the `attr!` tiers, plus a last tier for any `serde::Serialize` value.

`render!`, `attr!` and the other macros are `macro_rules!` macros. `#[macro_export]` places them at
the crate root as `__htmloxide_*`; `pub use` re-exports them under `__private`.

### Attribute contexts

`attr_context` picks an `AttrContext` from the attribute's name, lowercased, and the tag. htmx reads
`data-hx-*` as `hx-*`, so both get the same context.

| `AttrContext` | Attributes | Writer | Accepts |
|---|---|---|---|
| `Text` | any other | `attr!(.., Text)` | the `attr!` tiers |
| `Words` | `aria-*`, `data-*`, `draggable`, `spellcheck`, `contenteditable`, `hx-boost`, `hx-validate`, `hx-push-url`, `hx-replace-url`, `hx-history`, `hx-swap-oob` | `attr!(.., Words)` | the `attr!` tiers |
| `Url` | `href`, `src`, `action`, `formaction`, `xlink:href`, `poster`, `cite`, `data` | `attr!(.., Url)` | the `attr!` tiers; a `javascript:` or `vbscript:` URL becomes `about:invalid#blocked`, a `Js` value is written as `javascript:` + code |
| `Srcdoc` | `srcdoc` | `attr!(.., Srcdoc)` | the `attr!` tiers; the value is written as it would be between tags, then escaped again |
| `Json` | `hx-vals`, `hx-headers`, `hx-request` | `attr_json!` | the `attr_json!` tiers; a `Js` value is written as `js:` + code |
| `Js` | `on*`, `hx-on*`, `hx-vars` | `attr_js` | `Js`, `Option<Js>` |
| `StaticUrl` | `src` on `<script>`, `href` on `<base>` | `attr_static` | `&'static str`, `Option<&'static str>` |

`is_script_url` reads a URL as browsers do: leading spaces and control characters are skipped,
tabs and line breaks are ignored, and the scheme is matched in any case.

### Text and whitespace

Tokens carry no whitespace: `Hello world` reaches the macro as `Hello` and `world`. Each token's
span holds its line and column (stable in proc macros since Rust 1.88), and the whitespace is
rebuilt from them:

- `source_text` rebuilds unquoted text from the gaps between spans. The gaps to the neighbouring
  nodes, or to the enclosing tags, are included, so `Hi {name}` keeps its space. When positions
  are missing (0:0) or out of order (tokens from another macro), it falls back to the token
  stream's own printing.
- `collapse_line_breaks` then applies JSX's rule: each line is trimmed at its line breaks, blank
  lines are dropped, and the rest are joined with one space.
- `same_line_gap` applies JSX's rule between siblings that aren't text: a gap on one line is one
  space, a gap containing a line break is nothing.
- rstml reads `(..)` and `[..]` in text as one token each, so a `{..}` inside them would be printed
  as text. `reject_nested_blocks` reports it as an error.

### `<script>` and `<style>`

rstml keeps their content as raw tokens. `Generator::raw_text` accepts one string literal (code,
written at compile time), one `{..}` block (a `RawTextValue`), or anything else (code, rebuilt with
`source_text`). More than one literal or block is a compile error.

- Code from the source containing `</script` (`</style` in `<style>`), in any case, is a compile
  error, as is `<script` inside `<script>`: after a `<!--`, it makes the browser skip the next
  `</script>`.
- `script_value` takes `Js` (code) or any `serde::Serialize` value (data). Data is written by
  `serde_json`, then `<`, `>`, `&`, U+2028 and U+2029 are written as `\u` escapes.
- `style_value` takes any `AsRef<str>`, as CSS text.
- In `Js` and CSS written at runtime, `write_guarded` replaces the first letter of `</script` and
  `<script` (`</style` in `<style>`), in any case, with an escape for the same letter.
- rstml treats only lowercase `script` and `style` as raw text, so `<SCRIPT>` is a compile error.

### Control flow

`@if`, `@for`, `@match` and `@let` are rstml's custom node, `ControlFlow` in `control_flow.rs`.
rstml calls `ControlFlow::peek_element` at every node position; on `@` and a keyword,
`ControlFlow::parse_recoverable` parses the Rust parts with syn
(`Expr::parse_without_eager_brace`, as Rust does after `if`) and each `{..}` body as nodes.

```rust
html! { <ul>@for item in &items { <li>{item}</li> }</ul> }
```

expands to

```rust
buf.push_str("<ul>");
for item in &items {
    buf.push_str("<li>");
    ::htmloxide::__private::render!(&(item), &mut buf);
    buf.push_str("</li>");
}
buf.push_str("</ul>");
```

- Each body's parts are collected in their own list (`Generator::body`), so fixed HTML merges
  inside a body but never across its edges.
- Bodies have no enclosing tags, so their text starts at its first word and ends at its last.
  `@let` writes nothing and is skipped when finding a node's neighbours.
- Each construct is parsed on a syn fork. A syn error inside a `{..}` group leaves the group's
  remaining tokens marked as unexpected, which rstml turns into a panic once the parse ends; the
  fork discards that mark. The failed construct is reported, skipped (`skip_construct`), and the
  rest of the markup still parses.

### Markup inside `{..}`

rstml parses the inside of every `{..}` as Rust. `mark_markup`, registered as rstml's
`transform_block` hook, runs first: an inside that starts with `<` or `@` and doesn't parse as Rust
is replaced with `::htmloxide::__private::markup! { .. }`, a macro call that does. The generator
detects that call (`marked_markup`) and parses its tokens as markup:

- As a component prop: built into a separate `Markup` value (`markup_value`), like children. In
  `html_async!` it is built inside the same future.
- Between tags: written in place.
- As an HTML attribute value: a compile error.

The call never reaches the output; no `markup!` macro exists.

## `#[component]`

In `htmloxide-macros/src/component.rs`.

```rust
#[component]
fn Card(title: &str, #[prop(default)] featured: bool, children: Markup) -> Markup { .. }
```

generates

- `CardProps<'props>`, with one public field per argument. Elided lifetimes (`&str`, `'_`) are
  named `'props`, which is added to the generics when used.
- `CardPropsBuilder<title, featured, children>`, a type-state builder: each type parameter is
  `Unset` or `Set<T>`. The `Props` impl on `CardProps` returns the builder with every prop
  `Unset`.
  - The type parameters are named after the props, so rustc's error for an unknown prop lists the
    real ones. A prop named like a type that the generated code mentions (`char: char`), or a raw
    identifier, gets a positional name (`__F0`) instead.
  - Each setter declares only the generics that its prop type mentions, so the rest need not be
    inferred at the call.
  - Setter argument: `T`; `impl Into<T>` with `into`; `impl Into<Option<T>>` for `Option<T>`;
    `impl IntoOptionProp<T, K>` for `Option<T>` with `into`. `IntoOptionProp` has an impl for
    `U: Into<T>` and one for `Option<U>`; the marker `K` (`FromValue`, `FromOption`) keeps them
    from overlapping and is inferred from the one that applies.
  - A setter requires a per-prop trait implemented only for `Unset`, so a second call fails with
    "prop `x` is set more than once".
  - `build()` requires, for each required prop, a per-prop trait implemented only for `Set<T>`:
    "missing required prop `x` on `<Card>`". Other props go through `PropOrDefault`, implemented
    for `Set<T>` (the value) and for `Unset` (the default). An `Option<T>` prop's default is
    `None`.
  - `#[diagnostic::on_unimplemented]` gives these trait errors their messages.
- `fn Card(props: CardProps) -> Markup`: the original attributes, visibility, `async` and body,
  with `props` first destructured into the original argument names.

A tag is a component when its last path segment starts with an uppercase letter.
`<Card title="x">kids</Card>` lowers to

```rust
{ let c = &Card; c(props_builder(c).title("x").children({ /* kids */ }).build()) }
```

`props_builder` infers the props type from the function's signature, so `<ui::Card>` works without
importing `CardProps`. Each mention of a generic function is a separate instantiation, so `c` is
bound once and shared by `props_builder` and the call. `{title}` lowers to `.title(title)`; a prop
named like a keyword (`type="x"`) to the raw identifier setter (`.r#type("x")`).

## `html_async!`

The same generator, with the output in an `async move { .. }` block. Component calls and `{expr}`
values that aren't place expressions are taken by value through `render_async!`: an `IntoFuture`
value is awaited, then the result is written by `render!`. Place expressions (`{x}`, `{x.y}`,
`{x[i]}`, `{*x}`) stay borrowed (`block_is_place`).

`each_async` awaits its futures with `futures_util::future::join_all`: concurrently, in order.

## `js!`

In `htmloxide-macros/src/js.rs`. The template is parsed like `format!`'s: `{name}`, `{}`, `{0}`,
named arguments, `{{` / `}}`; every argument must be used. The expansion binds each argument once,
pushes the template's code into a `String`, and calls `js_part` for each value: `Js` is inserted as
code, any other `serde::Serialize` value as JSON. `js_from_template` wraps the result in `Js`. The
only other way to make a `Js` is `Js::from_static`, which takes a `&'static str`.

## Tests

In `htmloxide/tests/`:

- `basic.rs`: syntax, escaping, attributes, whitespace, `<script>` and `<style>`, `js!`
- `serde.rs`: `serde::Serialize` values in `<script>`, `js!` and `hx-vals`
- `control_flow.rs`: `@if`, `@for`, `@match`, `@let`, nesting, whitespace around and inside bodies
- `components.rs`: props, defaults, generics, lifetimes, paths, markup props
- `async_components.rs`: awaiting, nesting, concurrency timing
- `readme.rs`: the README's snippets
