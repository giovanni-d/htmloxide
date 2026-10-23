# Architecture

## Crates

| crate                    | role |
|--------------------------|------|
| `htmloxide-macros`       | proc macros: `html!`, `html_async!`, `#[component]` |
| `htmloxide`              | the library users depend on: re-exports the macros, `Markup`, `Render`, `each`, `each_async`, and the runtime helpers the generated code calls |
| `examples/basic`         | prints a static page |
| `examples/live-todos`    | axum + htmx + SSE shared todo list |

## `html!`: from tags to string writes

[rstml](https://crates.io/crates/rstml) parses the macro input into a node tree. The generator
walks it and emits a flat list of parts: static strings and dynamic expressions. Neighbouring
static parts are merged and static text is escaped at compile time.

```rust
html! { <a href={url} class="x">Hi {name}</a> }
```

expands to roughly

```rust
{
    let mut buf = String::with_capacity(20);                  // static bytes, known at compile time
    buf.push_str("<a");
    ::htmloxide::__private::attr!(" href", &(url), &mut buf); // runtime decision, see below
    buf.push_str(" class=\"x\">Hi ");
    ::htmloxide::__private::render!(&(name), &mut buf);       // escaped
    buf.push_str("</a>");
    ::htmloxide::PreEscaped(buf)                               // = Markup
}
```

`buf` is created with `Span::mixed_site()` hygiene, so user code can't collide with it.

### Picking behaviour by type

`render!` and `attr!` use *autoref specialization*: they call a method on `&&&&Wrap(value)`, and
method resolution picks the impl that needs the fewest auto-derefs, i.e. the first tier whose
bounds hold.

- **`{expr}` children**: `Render` -> iterable of `Render` (`Vec`, slices, `Option`) -> future
  (compile error suggesting `html_async!`) -> `Display`
- **`key={expr}` attributes**: `bool` (adds or drops the attribute) -> `Option<impl Render>`
  (dropped on `None`) -> `Render` -> `Display`

`{name}` is emitted as `&(name)`, never `&({name})`, because a block expression would *move* the
value before borrowing the temporary.

### Text and whitespace

- Unquoted text (`<p>Hello world</p>`) keeps its spaces through rstml's `rawtext-stable-hack`,
  which re-reads the macro's source text on stable Rust. If `html!` is produced by another macro,
  spacing falls back to token spacing.
- Between non-text siblings, the JSX rule applies: `{a} {b}` keeps one space when the gap is on
  a single line, and a gap containing a newline is dropped. This uses span line/column
  information (stable since Rust 1.88).
- `<script>` / `<style>` content is raw text: no escaping and no tag parsing. A lone `{expr}` in
  them still goes through `Render`, so pass `PreEscaped(..)` for trusted CSS/JS.

## `#[component]`

```rust
#[component]
fn Card(title: &str, #[prop(default)] featured: bool, children: Markup) -> Markup { .. }
```

generates

- `CardProps<'props>` with one public field per argument. Elided lifetimes (`&str`, `'_`) are
  rewritten to `'props`.
- `CardPropsBuilder<F0, F1, F2>`: a type-state builder where each `Fi` is `Unset` or `Set<T>`.
  - Each setter carries only the generics its prop's type mentions, so they stay inferable.
  - A per-prop marker trait implemented only for `Unset` rejects setting a prop twice:
    "prop `x` is set more than once".
  - `build()` requires a per-prop marker trait implemented only for `Set<T>` on required props:
    "missing required prop `x` on `<Card>`". Defaulted props go through `PropOrDefault`.
- `fn Card(props: CardProps) -> Markup`: the original body, after destructuring `props` into the
  original argument names. `async fn` stays `async`.

A tag is a component when its last path segment starts with an uppercase letter.
`<Card title="x">kids</Card>` lowers to

```rust
{ let c = &Card; c(props_builder(c).title("x").children({ /* kids */ }).build()) }
```

`props_builder` infers the props type from the function's signature, so `<ui::Card>` works
without importing `CardProps`. Binding `c` once makes generic parameters infer from a single
instantiation.

## `html_async!`

Same generator, but the output is wrapped in `async move { .. }`, and component results and
non-place `{expr}` values are taken by value through `render_async!`:

- If the value is `IntoFuture`, it is awaited.
- The result is then rendered like `render!`.

Place expressions (`{x}`, `{x.y}`, `{x[i]}`, `{*x}`) stay borrowed. `each_async` uses
`futures_util::future::join_all`, so items load concurrently and keep their order.

## Runtime types

`Markup`, `Render`, `PreEscaped` and the escaping rules are re-exported from
[maud](https://maud.lambda.xyz). Anything implementing maud's `Render` works inside `{..}`, and
`Markup` values from either crate can be mixed.

## Tests

- `tests/basic.rs`: syntax, escaping, attributes, whitespace
- `tests/components.rs`: props, defaults, generics, lifetimes, paths
- `tests/async_components.rs`: awaiting, nesting, concurrency timing
- `tests/readme.rs`: mirrors the README snippets
