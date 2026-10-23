//! The procedural macros behind `htmloxide`: `html!`, `html_async!` and
//! `#[component]`.
//!
//! Users depend on the `htmloxide` crate, which re-exports these macros. The
//! code generated here calls into `htmloxide::__private`, so this crate is
//! never used on its own.
//!
//! A procedural macro is a function the compiler runs while it compiles the
//! user's crate. It receives the tokens written inside `html! { .. }` and
//! returns the Rust code that takes their place. For example,
//!
//! ```ignore
//! html! { <a href={url} class="x">Hi {name}</a> }
//! ```
//!
//! becomes roughly
//!
//! ```ignore
//! {
//!     let mut buf = String::with_capacity(20); // total length of the static HTML
//!     buf.push_str("<a");
//!     ::htmloxide::__private::attr!(" href", &(url), &mut buf);
//!     buf.push_str(" class=\"x\">Hi ");
//!     ::htmloxide::__private::render!(&(name), &mut buf);
//!     buf.push_str("</a>");
//!     ::htmloxide::PreEscaped(buf)
//! }
//! ```
//!
//! The expansion of `html!` and `html_async!` has three stages:
//!
//! 1. **Parse.** [rstml](https://crates.io/crates/rstml) reads the tokens into
//!    a tree of nodes: elements, text, `{..}` blocks, comments.
//! 2. **Walk.** `Generator` visits the tree and records a flat list of
//!    `Part`s: either fixed HTML, or a Rust expression to render at runtime.
//!    Fixed text is escaped right away and neighbouring fixed parts are
//!    merged, so each run of static HTML becomes a single string literal.
//! 3. **Emit.** `Generator::finish` turns each part into one statement that
//!    appends to a `String`.

use std::collections::HashSet;

mod component;

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use proc_macro2_diagnostics::{Diagnostic, Level};
use quote::{ToTokens, quote, quote_spanned};
use rstml::node::{
    KeyedAttribute, KeyedAttributeValue, Node, NodeAttribute, NodeBlock, NodeElement, NodeName,
};
use rstml::{Parser, ParserConfig};
use syn::spanned::Spanned;
use syn::{Expr, Lit};

// --------------------------------------------------
// Data: elements with special parsing rules
// --------------------------------------------------

/// HTML void elements: never have children and never get a closing tag.
///
/// rstml is configured with this list, so `<br>` and `<input ..>` need no
/// `/>` in the macro input.
const VOID_ELEMENTS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Elements whose content is emitted verbatim (no escaping, no child parsing).
///
/// CSS and JavaScript use characters such as `<`, `>` and `&` that have to
/// reach the browser unchanged.
const RAW_TEXT_ELEMENTS: [&str; 2] = ["script", "style"];

// --------------------------------------------------
// Entry points: html!, html_async! and #[component]
// --------------------------------------------------

/// Builds `Markup` from HTML-like syntax. Values in `{..}` are HTML-escaped.
#[proc_macro]
pub fn html(input: TokenStream) -> TokenStream {
    expand(input, false)
}

/// Like `html!`, but evaluates to `impl Future<Output = Markup>` and awaits
/// async components (and any future produced by a `{..}` block).
#[proc_macro]
pub fn html_async(input: TokenStream) -> TokenStream {
    expand(input, true)
}

/// The shared body of `html!` and `html_async!`.
fn expand(input: TokenStream, is_async: bool) -> TokenStream {
    // What the macro call looks like in the user's source; `%%` stands for
    // its input.
    let pattern = if is_async {
        quote!(html_async! {%%})
    } else {
        quote!(html! {%%})
    };
    let config = ParserConfig::new()
        // Keep a `{..}` block that isn't valid Rust yet (`{user.}` while
        // typing) as a node instead of failing, so rust-analyzer can still
        // offer completions inside it.
        .recover_block(true)
        .always_self_closed_elements(HashSet::from(VOID_ELEMENTS))
        .raw_text_elements(HashSet::from(RAW_TEXT_ELEMENTS))
        // Lets rstml re-read the original source to keep whitespace in
        // unquoted text like `<p>Hello world</p>` on stable Rust. Tokens
        // alone don't record the spaces between words.
        .macro_call_pattern(pattern);

    // Parse as much as possible instead of stopping at the first mistake:
    // `nodes` is everything that could be read, `parse_diagnostics` the
    // errors found on the way.
    let (nodes, parse_diagnostics) = Parser::new(config).parse_recoverable(input).split_vec();

    let mut generator = Generator::new(is_async);
    generator.nodes(&nodes, Context::Normal);

    // A proc macro reports errors by placing `compile_error!` calls in its
    // output. `emit_as_expr_tokens` builds one per diagnostic, pointing at
    // the user's code.
    let errors = parse_diagnostics
        .into_iter()
        .chain(std::mem::take(&mut generator.diagnostics))
        // Warnings can't be emitted on stable; only surface real errors.
        .filter(|d| d.level() == Level::Error)
        .map(Diagnostic::emit_as_expr_tokens)
        .collect::<Vec<_>>();

    // The generated body is emitted next to the errors, so the user's `{..}`
    // expressions are still type-checked and completed by the IDE.
    let body = generator.finish();
    if is_async {
        // `async move`, so the future can be returned or used inside a
        // `.map(|item| html_async!{..})` closure. Borrow first
        // (`let s = &state;`) to keep using a value afterwards.
        quote!(async move {
            #( #errors; )*
            #body
        })
    } else {
        // A block, so the generated statements form a single expression.
        quote!({
            #( #errors; )*
            #body
        })
    }
    .into()
}

/// Turns a function into a component usable as `<Name prop=..>children</Name>`
/// inside `html!`. The function's arguments become the tag's props.
#[proc_macro_attribute]
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    // `attr` holds the tokens inside `#[component(..)]`; there are no options.
    if !attr.is_empty() {
        let attr = TokenStream2::from(attr);
        return syn::Error::new_spanned(attr, "`#[component]` takes no arguments")
            .to_compile_error()
            .into();
    }
    // Anything other than a function is a compile error here.
    let item = syn::parse_macro_input!(item as syn::ItemFn);
    component::expand(item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

// --------------------------------------------------
// Data: the generator's state and output
// --------------------------------------------------

/// How text in the current element is written out.
#[derive(Clone, Copy, PartialEq)]
enum Context {
    /// Text is HTML-escaped.
    Normal,
    /// Inside `<script>` / `<style>`: text is not escaped.
    RawText,
}

/// One piece of the generated output. Each part becomes one statement, in
/// order.
///
/// `span` is the location of the user's code that produced the part. The
/// generated statement carries it, so a type error points at the user's
/// `{expr}` rather than at the macro.
enum Part {
    /// Fixed HTML, already escaped. Neighbouring static text is merged into a
    /// single `Static`.
    Static(String),
    /// `{expr}` child, component or `{expr}` spread attribute: rendered
    /// via `Render`. `owned` (async mode only): evaluated by value and
    /// awaited first if it's a future.
    Render {
        expr: TokenStream2,
        span: Span,
        owned: bool,
    },
    /// `key={expr}`: bool / Option / value, decided at runtime.
    Attr {
        key: String,
        expr: TokenStream2,
        span: Span,
    },
}

/// Walks the rstml node tree and collects the output parts.
struct Generator {
    parts: Vec<Part>,
    /// Errors found while walking. They're collected rather than returned, so
    /// the walk goes on and one compile reports every mistake, not only the
    /// first.
    diagnostics: Vec<Diagnostic>,
    /// `html_async!`: values are taken by value and futures are awaited.
    is_async: bool,
}

impl Generator {
    fn new(is_async: bool) -> Self {
        Self {
            parts: Vec::new(),
            diagnostics: Vec::new(),
            is_async,
        }
    }

    // --------------------------------------------------
    // Generator: adding parts
    // --------------------------------------------------

    /// Appends fixed HTML, extending the previous part when it's static too.
    fn push_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        match self.parts.last_mut() {
            Some(Part::Static(last)) => last.push_str(s),
            _ => self.parts.push(Part::Static(s.to_owned())),
        }
    }

    /// Appends fixed text that needs escaping. The text is known at compile
    /// time, so it's escaped at compile time too.
    fn push_escaped(&mut self, s: &str) {
        self.push_str(&escape(s));
    }

    /// Appends an expression to render at runtime.
    fn push_render(&mut self, expr: TokenStream2, span: Span, owned: bool) {
        self.parts.push(Part::Render { expr, span, owned });
    }

    fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::spanned(span, Level::Error, msg.into()));
    }

    // --------------------------------------------------
    // Generator: walking the node tree
    // --------------------------------------------------

    /// Writes sibling nodes in order. In normal text, siblings separated by a
    /// gap on the same line (`{a} {b}`) keep one space between them; a line
    /// break between siblings is only formatting and is dropped.
    fn nodes(&mut self, nodes: &[Node], ctx: Context) {
        let mut prev: Option<&Node> = None;
        for node in nodes {
            if ctx == Context::Normal
                && let Some(prev) = prev
                && same_line_gap(prev, node)
            {
                self.push_str(" ");
            }
            self.node(node, ctx);
            prev = Some(node);
        }
    }

    /// Writes one node of any kind.
    fn node(&mut self, node: &Node, ctx: Context) {
        match node {
            // `<div>..</div>` or `<Card>..</Card>`.
            Node::Element(el) => self.element(el),
            // `<>..</>` groups nodes without adding a tag of its own.
            Node::Fragment(frag) => self.nodes(&frag.children, ctx),
            // Quoted text: `<p>"don't"</p>`.
            Node::Text(text) => {
                let value = text.value_string();
                match ctx {
                    Context::Normal => self.push_escaped(&value),
                    Context::RawText => self.push_str(&value),
                }
            }
            // Unquoted text: `<p>Hello world</p>`. Inside `<script>` /
            // `<style>`, the element's whole content is one `RawText` node.
            Node::RawText(raw) => {
                // The original source text when available (keeps spaces),
                // otherwise the tokens printed back as text.
                let text = raw.to_string_best();
                match ctx {
                    Context::Normal => self.push_escaped(&text),
                    Context::RawText => {
                        // `<style>{expr}</style>`: a lone block is an
                        // expression, rendered through `Render` (escaped
                        // unless it's `PreEscaped`).
                        if let Ok(block) = syn::parse2::<syn::Block>(raw.to_token_stream()) {
                            let expr = match block.stmts.as_slice() {
                                [syn::Stmt::Expr(inner, None)] => inner.to_token_stream(),
                                _ => block.to_token_stream(),
                            };
                            let owned = self.is_async;
                            self.push_render(expr, block.span(), owned);
                            return;
                        }
                        // `<script>"..."</script>`: a single string literal is
                        // the escape hatch for JS/CSS that Rust can't tokenize.
                        match syn::parse_str::<syn::LitStr>(text.trim()) {
                            Ok(lit) => self.push_str(&lit.value()),
                            Err(_) => self.push_str(&text),
                        }
                    }
                }
            }
            Node::Block(block) => {
                // `{name}` / `{user.name}` stay borrowed; anything else is a
                // temporary we can take by value (and await, in async mode).
                let owned = self.is_async && !block_is_place(block);
                self.push_render(block_tokens(block), block.span(), owned);
            }
            // `<!-- "text" -->`: a quoted string loses its quotes; anything
            // else is copied as written.
            Node::Comment(comment) => {
                let text = comment.value.to_string_best();
                let text = syn::parse_str::<syn::LitStr>(text.trim())
                    .map(|l| l.value())
                    .unwrap_or(text);
                self.push_str("<!-- ");
                // `-->` in the text would close the HTML comment early.
                self.push_str(&text.replace("-->", "--&gt;"));
                self.push_str(" -->");
            }
            // `<!DOCTYPE html>`, copied as written.
            Node::Doctype(doctype) => {
                self.push_str("<!DOCTYPE ");
                self.push_str(doctype.value.to_string_best().trim());
                self.push_str(">");
            }
            Node::Custom(_) => unreachable!("no custom nodes configured"),
        }
    }

    // --------------------------------------------------
    // Generator: HTML elements
    // --------------------------------------------------

    /// Writes an HTML element such as `<div class="x">..</div>`, or calls
    /// `component` for a component tag.
    fn element(&mut self, el: &NodeElement<rstml::Infallible>) {
        // A tag whose last path segment starts with an uppercase letter
        // (`Card`, `ui::Card`) is a component, as in JSX.
        if let NodeName::Path(path) = el.name() {
            let is_component = path.path.segments.last().is_some_and(|s| {
                s.ident
                    .to_string()
                    .starts_with(|c: char| c.is_ascii_uppercase())
            });
            if is_component {
                return self.component(el, path);
            }
        }

        // HTML tag names are case-insensitive, so `<BR>` is a void element
        // too. The tag itself is written as typed.
        let name = el.name().to_string();
        let lower = name.to_ascii_lowercase();
        let is_void = VOID_ELEMENTS.contains(&lower.as_str());

        // `<{tag}>`: tag names are written into the static HTML, so they have
        // to be known at compile time.
        if let NodeName::Block(block) = el.name() {
            self.error(block.span(), "dynamic tag names are not supported");
            return;
        }

        self.push_str("<");
        self.push_str(&name);
        for attr in el.attributes() {
            self.attribute(attr);
        }
        self.push_str(">");

        // A void element ends with its opening tag: no children, no closing
        // tag.
        if is_void {
            if !el.children.is_empty() {
                self.error(
                    el.name().span(),
                    format!("<{name}> is a void element and cannot have children"),
                );
            }
            return;
        }

        let ctx = if RAW_TEXT_ELEMENTS.contains(&lower.as_str()) {
            Context::RawText
        } else {
            Context::Normal
        };
        self.nodes(&el.children, ctx);

        self.push_str("</");
        self.push_str(&name);
        self.push_str(">");
    }

    // --------------------------------------------------
    // Generator: component tags
    // --------------------------------------------------

    /// Writes a component tag as a call to the component function:
    ///
    /// `<Card title="x" featured>kids</Card>` ->
    /// `Card(props_builder(&Card).title("x").featured(true).children(html!{kids}).build())`
    ///
    /// Each attribute becomes a call to the setter of the same name on the
    /// props builder generated by `#[component]`, and the children become the
    /// `children` prop.
    fn component(&mut self, el: &NodeElement<rstml::Infallible>, path: &syn::ExprPath) {
        let tag_span = path.span();
        let mut setters = Vec::new();

        for attr in el.attributes() {
            // `<Card {..}>` has no prop name to turn into a setter call.
            let attr = match attr {
                NodeAttribute::Attribute(a) => a,
                NodeAttribute::Block(b) => {
                    self.error(b.span(), "spread attributes aren't supported on components");
                    continue;
                }
            };
            // The prop name is used as a method name, so it has to be a plain
            // identifier. A Rust keyword becomes a raw identifier, matching an
            // argument declared as `r#type`.
            let key_span = attr.key.span();
            let key_str = attr.key.to_string();
            let key = match &attr.key {
                NodeName::Path(p) if p.path.get_ident().is_some() => {
                    let id = p.path.get_ident().unwrap();
                    // `type=..` -> `.r#type(..)`
                    if syn::parse_str::<syn::Ident>(&key_str).is_ok() {
                        id.clone()
                    } else {
                        syn::Ident::new_raw(&key_str, id.span())
                    }
                }
                _ => {
                    self.error(
                        key_span,
                        format!("component props must be Rust identifiers, found `{key_str}`"),
                    );
                    continue;
                }
            };
            // `<Card featured>` means `featured=true`; `title="x"` and
            // `title={expr}` pass the expression to the setter.
            let value = match &attr.possible_value {
                KeyedAttributeValue::None => quote_spanned!(key_span=> true),
                KeyedAttributeValue::Binding(b) => {
                    self.error(b.span(), "closure bindings (`attr(x)`) are not supported");
                    continue;
                }
                KeyedAttributeValue::Value(_) => match attr.value() {
                    Some(expr) => unbrace(expr),
                    None => continue, // invalid block, already reported by rstml
                },
            };
            setters.push(quote_spanned!(key_span=> .#key(#value)));
        }

        // The children get a generator of their own and become a separate
        // `Markup` value, passed as the `children` prop.
        if !el.children.is_empty() {
            let mut child = Generator::new(self.is_async);
            child.nodes(&el.children, Context::Normal);
            self.diagnostics.append(&mut child.diagnostics);
            let body = child.finish();
            setters.push(quote_spanned!(tag_span=> .children({ #body })));
        }

        // Bind the component once so its generic params are inferred from a
        // single instantiation (both for `props_builder` and for the call).
        // `mixed_site` keeps the binding's name out of reach of user code.
        let c = syn::Ident::new("__htmloxide_component", Span::mixed_site());
        let expr = quote_spanned!(tag_span=> {
            let #c = &#path;
            #c(::htmloxide::__private::props_builder(#c) #( #setters )* .build())
        });
        // In `html_async!` the call's result is taken by value, so an async
        // component's future gets awaited.
        let owned = self.is_async;
        self.push_render(expr, tag_span, owned);
    }

    // --------------------------------------------------
    // Generator: attributes
    // --------------------------------------------------

    /// Writes one attribute of an HTML element.
    fn attribute(&mut self, attr: &NodeAttribute) {
        match attr {
            // `<input {"disabled"}>`: a computed attribute string, rendered
            // through `Render`. Never awaited, even in `html_async!`.
            NodeAttribute::Block(block) => {
                self.push_str(" ");
                self.push_render(block_tokens(block), block.span(), false);
            }
            NodeAttribute::Attribute(attr) => self.keyed_attribute(attr),
        }
    }

    /// Writes `key`, `key=value` or `key={expr}`. The key is written exactly as
    /// typed, so `hx-post`, `data-id` and `aria-label` work as they are.
    fn keyed_attribute(&mut self, attr: &KeyedAttribute) {
        let key = attr.key.to_string();

        match &attr.possible_value {
            // `<input disabled>`
            KeyedAttributeValue::None => {
                self.push_str(" ");
                self.push_str(&key);
            }
            KeyedAttributeValue::Binding(b) => {
                self.error(b.span(), "closure bindings (`attr(x)`) are not supported");
            }
            KeyedAttributeValue::Value(_) => {
                let Some(expr) = attr.value() else {
                    // Invalid braced block; rstml already reported it.
                    return;
                };
                // A literal value is handled now, at compile time; only
                // `{expr}` needs code that runs later.
                match literal(expr) {
                    // `disabled=true` / `disabled=false`
                    Some(Literal::Bool(true)) => {
                        self.push_str(" ");
                        self.push_str(&key);
                    }
                    Some(Literal::Bool(false)) => {}
                    // `class="card"`: fully static, escaped at compile time.
                    Some(Literal::Text(text)) => {
                        self.push_str(" ");
                        self.push_str(&key);
                        self.push_str("=\"");
                        self.push_escaped(&text);
                        self.push_str("\"");
                    }
                    // `class={expr}`: decided at runtime. A bool toggles the
                    // attribute, an Option omits it on None, and anything
                    // else is rendered (escaped) as the value.
                    None => {
                        self.parts.push(Part::Attr {
                            key,
                            expr: unbrace(expr),
                            span: expr.span(),
                        });
                    }
                }
            }
        }
    }

    // --------------------------------------------------
    // Generator: emitting the code
    // --------------------------------------------------

    /// Turns the collected parts into the macro's output: statements that fill
    /// a `String`, ending in `PreEscaped(buf)`, which is a `Markup`.
    fn finish(self) -> TokenStream2 {
        // The buffer starts with room for all the static HTML; dynamic values
        // may grow it further.
        let static_len: usize = self
            .parts
            .iter()
            .map(|p| if let Part::Static(s) = p { s.len() } else { 0 })
            .sum();

        // Created by this macro with `mixed_site` hygiene, so it can never
        // clash with (or be named by) user code inside `{ ... }` blocks.
        let buf = syn::Ident::new("__htmloxide_buf", Span::mixed_site());

        // A proc macro sees tokens, not types, so it can't tell whether
        // `{expr}` is a `bool`, an `Option` or a future. The generated code
        // calls the `render!`, `render_async!` and `attr!` macros, which make
        // that choice when the user's crate is type-checked.
        let stmts = self.parts.into_iter().map(|part| match part {
            Part::Static(s) => quote!(#buf.push_str(#s);),
            Part::Render {
                expr,
                span,
                owned: false,
            } => quote_spanned!(span=>
                ::htmloxide::__private::render!(&(#expr), &mut #buf);
            ),
            Part::Render {
                expr,
                span,
                owned: true,
            } => quote_spanned!(span=>
                ::htmloxide::__private::render_async!(#expr, &mut #buf);
            ),
            Part::Attr { key, expr, span } => {
                let key = format!(" {key}");
                quote_spanned!(span=>
                    ::htmloxide::__private::attr!(#key, &(#expr), &mut #buf);
                )
            }
        });

        // `PreEscaped` marks the string as HTML that must not be escaped
        // again; `Markup` is `PreEscaped<String>`.
        quote!(
            let mut #buf = ::std::string::String::with_capacity(#static_len);
            #( #stmts )*
            ::htmloxide::PreEscaped(#buf)
        )
    }
}

// --------------------------------------------------
// Helpers: reading {..} blocks and literals
// --------------------------------------------------

/// `{x + 1}` -> `x + 1`, so passing it as a method argument doesn't trip
/// the `unused_braces` lint. Multi-statement blocks are kept as blocks.
fn unbrace(expr: &Expr) -> TokenStream2 {
    if let Expr::Block(b) = expr
        && let [syn::Stmt::Expr(inner, None)] = b.block.stmts.as_slice()
        && b.attrs.is_empty()
        && b.label.is_none()
    {
        return inner.to_token_stream();
    }
    expr.to_token_stream()
}

/// An attribute value fully known at compile time.
enum Literal {
    Text(String),
    Bool(bool),
}

/// Reads `expr` as a literal attribute value. Integers are written in base 10
/// without their suffix (`0x1F_u8` -> `31`). Other kinds of literal, such as
/// byte strings, return `None` and are rendered at runtime like any other
/// expression.
fn literal(expr: &Expr) -> Option<Literal> {
    let Expr::Lit(lit) = expr else { return None };
    Some(match &lit.lit {
        Lit::Str(s) => Literal::Text(s.value()),
        Lit::Char(c) => Literal::Text(c.value().to_string()),
        Lit::Int(i) => Literal::Text(i.base10_digits().to_owned()),
        Lit::Float(f) => Literal::Text(f.base10_digits().to_owned()),
        Lit::Bool(b) => Literal::Bool(b.value),
        _ => return None,
    })
}

/// The expression inside `{ .. }`.
///
/// `{name}` must become `name`, not `{name}`: a block expression *moves* its
/// value, so `&({name})` would move `name` before borrowing the temporary.
/// Multi-statement blocks stay blocks; syn-invalid blocks are emitted as
/// written so rust-analyzer still gets something to complete against.
fn block_tokens(block: &NodeBlock) -> TokenStream2 {
    match block {
        NodeBlock::ValidBlock(b) => match b.stmts.as_slice() {
            [syn::Stmt::Expr(inner, None)] => inner.to_token_stream(),
            _ => b.to_token_stream(),
        },
        NodeBlock::Invalid(_) => block.to_token_stream(),
    }
}

// --------------------------------------------------
// Helpers: whitespace between siblings
// --------------------------------------------------

/// JSX whitespace rule for siblings that aren't text: `{a} {b}` and
/// `<b>x</b> {y}` keep one space when the gap is on a single line; a gap
/// containing a newline is just formatting and is dropped. (Unquoted text
/// already carries its own surrounding whitespace.)
fn same_line_gap(prev: &Node, next: &Node) -> bool {
    use proc_macro2::TokenTree;
    if matches!(prev, Node::RawText(_)) || matches!(next, Node::RawText(_)) {
        return false;
    }
    // Where `prev` ends. A `{..}` block is a single token (a group), whose
    // end is its closing brace.
    let last = prev
        .to_token_stream()
        .into_iter()
        .last()
        .map(|tt| match tt {
            TokenTree::Group(g) => g.span_close(),
            tt => tt.span(),
        });
    // Where `next` starts.
    let first = next
        .to_token_stream()
        .into_iter()
        .next()
        .map(|tt| tt.span());
    let (Some(last), Some(first)) = (last, first) else {
        return false;
    };
    let (end, start) = (last.end(), first.start());
    // Positions are 0:0 when unavailable; then fall back to "no space".
    end.line != 0 && end.line == start.line && start.column > end.column
}

// --------------------------------------------------
// Helpers: borrowed or owned values
// --------------------------------------------------

/// Is `{..}` a place expression (`x`, `x.y`, `x[i]`, `*x`)? Those are
/// borrowed rather than moved, matching what `{x}` means in `html!`.
///
/// Only `html_async!` asks: there, every other expression is taken by value,
/// so that a future can be awaited.
fn block_is_place(block: &NodeBlock) -> bool {
    fn is_place(e: &Expr) -> bool {
        match e {
            Expr::Path(_) | Expr::Field(_) | Expr::Index(_) => true,
            Expr::Paren(p) => is_place(&p.expr),
            Expr::Group(g) => is_place(&g.expr),
            Expr::Unary(u) => matches!(u.op, syn::UnOp::Deref(_)),
            _ => false,
        }
    }
    match block {
        NodeBlock::ValidBlock(b) => match b.stmts.as_slice() {
            [syn::Stmt::Expr(inner, None)] => is_place(inner),
            _ => false,
        },
        // Invalid code is already an error; borrowing it generates the same
        // code `html!` would.
        NodeBlock::Invalid(_) => true,
    }
}

// --------------------------------------------------
// Helpers: escaping
// --------------------------------------------------

/// Same escaping rules as maud's runtime escaper. Static text is escaped here,
/// at compile time; values in `{..}` are escaped by maud at runtime.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}
