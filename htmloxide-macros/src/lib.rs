//! The procedural macros of `htmloxide`. The generated code calls into
//! `htmloxide::__private`, so this crate is only used through `htmloxide`.
//!
//! # From markup to HTML
//!
//! A procedural macro is a function the compiler runs during compilation. It
//! receives the tokens written inside the macro call and returns the code
//! that replaces the call. For `html!`, that code builds the HTML into a
//! `String`:
//!
//! ```ignore
//! html! { <a href={url} class="x">Hi {name}</a> }
//! ```
//!
//! expands to
//!
//! ```ignore
//! {
//!     let mut buf = String::with_capacity(20);
//!     buf.push_str("<a");
//!     ::htmloxide::__private::attr!(" href", &(url), &mut buf, Url);
//!     buf.push_str(" class=\"x\">Hi ");
//!     ::htmloxide::__private::render!(&(name), &mut buf);
//!     buf.push_str("</a>");
//!     ::htmloxide::PreEscaped(buf)
//! }
//! ```
//!
//! The expansion has three stages:
//!
//! 1. **Parse.** [rstml](https://crates.io/crates/rstml) reads the tokens
//!    into a tree of nodes: elements, text, `{..}` blocks, and the control
//!    flow nodes of `control_flow.rs`.
//! 2. **Walk.** `Generator` visits the tree in document order and records a
//!    list of `Part`s. Everything written literally (tags, attribute names,
//!    text, literal attribute values) is known now, so it is escaped now and
//!    merged into fixed HTML. Each `{..}` becomes a part that holds the
//!    user's expression, to be written at runtime.
//! 3. **Emit.** `Generator::finish` turns each part into one statement that
//!    appends to the buffer. The value of the block is a `Markup`: a
//!    `String` marked as HTML, so it is not escaped again when it is placed
//!    inside other markup.
//!
//! The output is well-formed HTML because its structure comes only from the
//! source: rstml rejects unclosed and mismatched tags, and tag and attribute
//! names are never computed at runtime. A runtime value only fills text or an
//! attribute value, and is escaped there: in text, `<` becomes `&lt;` and
//! cannot start a tag; in an attribute value, `"` becomes `&quot;` and cannot
//! end the value.

use std::collections::HashSet;

mod component;
mod control_flow;
mod js;

use proc_macro::TokenStream;
use proc_macro2::{LineColumn, Span, TokenStream as TokenStream2};
use proc_macro2_diagnostics::{Diagnostic, Level};
use quote::{ToTokens, quote, quote_spanned};
use rstml::node::{KeyedAttribute, KeyedAttributeValue, NodeAttribute, NodeBlock, NodeName};
use rstml::{Parser, ParserConfig};
use syn::ext::IdentExt;
use syn::parse::ParseStream;
use syn::spanned::Spanned;
use syn::{Expr, Lit};

use control_flow::{Body, ControlFlow};

type Node = rstml::node::Node<ControlFlow>;
type NodeElement = rstml::node::NodeElement<ControlFlow>;

const VOID_ELEMENTS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

const RAW_TEXT_ELEMENTS: [&str; 2] = ["script", "style"];

const TRUE_FALSE_ATTRIBUTES: [&str; 3] = ["contenteditable", "draggable", "spellcheck"];

const HTMX_TRUE_FALSE_ATTRIBUTES: [&str; 6] = [
    "hx-boost",
    "hx-validate",
    "hx-push-url",
    "hx-replace-url",
    "hx-history",
    "hx-swap-oob",
];

/// Builds a `Markup` value from HTML written as Rust tokens. Each `{expr}`
/// is escaped for the place where it appears.
#[proc_macro]
pub fn html(input: TokenStream) -> TokenStream {
    expand(input, false)
}

/// Like `html!`, but evaluates to `impl Future<Output = Markup>` and awaits
/// async components and futures produced by `{..}` blocks.
#[proc_macro]
pub fn html_async(input: TokenStream) -> TokenStream {
    expand(input, true)
}

fn expand(input: TokenStream, is_async: bool) -> TokenStream {
    let (nodes, parse_diagnostics) = Parser::new(parser_config())
        .parse_recoverable(input)
        .split_vec();

    let mut generator = Generator::new(is_async);
    generator.nodes(&nodes, Context::Normal, None, None);

    // A procedural macro reports an error by emitting `compile_error!`
    // spanned to the faulty code. The generated code is emitted alongside,
    // so the user's expressions are still type-checked and completed by the
    // IDE.
    let errors = parse_diagnostics
        .into_iter()
        .chain(std::mem::take(&mut generator.diagnostics))
        .filter(|d| d.level() == Level::Error)
        .map(Diagnostic::emit_as_expr_tokens)
        .collect::<Vec<_>>();

    let body = generator.finish();
    if is_async {
        // `async move` lets the future be returned or built inside a closure.
        // It takes ownership of what it uses, so a value needed afterwards is
        // borrowed before the macro call (`let s = &state;`).
        quote!(async move {
            #( #errors; )*
            #body
        })
    } else {
        quote!({
            #( #errors; )*
            #body
        })
    }
    .into()
}

/// Turns a function into a component, used as `<Name prop=..>children</Name>`
/// inside `html!`. Each argument is a prop of the same name.
#[proc_macro_attribute]
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        let attr = TokenStream2::from(attr);
        return syn::Error::new_spanned(attr, "`#[component]` takes no arguments")
            .to_compile_error()
            .into();
    }
    let item = syn::parse_macro_input!(item as syn::ItemFn);
    component::expand(item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Builds a `Js` value from a template: `js!("select({id})")`. The template
/// is code; each `{..}` is a value written as JSON.
#[proc_macro]
pub fn js(input: TokenStream) -> TokenStream {
    js::expand(input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[derive(Clone, Copy, PartialEq)]
enum Context {
    Normal,
    RawText(RawTextElement),
}

/// What an attribute's value means to the browser, from the attribute's
/// name. Each kind has its own runtime writer, which decides the types it
/// accepts and how it escapes them.
#[derive(Clone, Copy)]
enum AttrContext {
    Text,
    Words,
    Js,
    Url,
    StaticUrl,
    Json,
    Srcdoc,
}

fn attr_context(element: &str, key: &str) -> AttrContext {
    let key = key.to_ascii_lowercase();
    // htmx reads `data-hx-*` as `hx-*`.
    let name = key
        .strip_prefix("data-")
        .filter(|k| k.starts_with("hx-"))
        .unwrap_or(&key);
    match (element, name) {
        ("script", "src") | ("base", "href") => AttrContext::StaticUrl,
        (_, name) if name.starts_with("on") || name.starts_with("hx-on") || name == "hx-vars" => {
            AttrContext::Js
        }
        (_, "hx-vals" | "hx-headers" | "hx-request") => AttrContext::Json,
        (_, "srcdoc") => AttrContext::Srcdoc,
        (
            _,
            "href" | "src" | "action" | "formaction" | "xlink:href" | "poster" | "cite" | "data",
        ) => AttrContext::Url,
        _ if takes_true_false(&key) => AttrContext::Words,
        _ => AttrContext::Text,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum RawTextElement {
    Script,
    Style,
}

impl RawTextElement {
    fn tag(self) -> &'static str {
        match self {
            RawTextElement::Script => "script",
            RawTextElement::Style => "style",
        }
    }
}

/// One statement of the generated code.
///
/// `span` points at the user's code that produced the part. The statement is
/// emitted with that span, so a type error is reported at the user's
/// `{expr}` instead of at the macro call.
enum Part {
    /// Fixed HTML, already escaped.
    Static(String),
    /// With `owned` (`html_async!` only), the value is moved instead of
    /// borrowed, so that a future can be awaited.
    Render {
        expr: TokenStream2,
        span: Span,
        owned: bool,
    },
    Attr {
        key: String,
        expr: TokenStream2,
        span: Span,
        ctx: AttrContext,
    },
    RawTextValue {
        expr: TokenStream2,
        span: Span,
        element: RawTextElement,
    },
    /// `@if` and `@for`. The pairs are emitted as `head { body }` one after
    /// another, so the heads `if a`, `else if b` and `else` form one chain.
    Branches(Vec<(TokenStream2, Vec<Part>)>),
    Match {
        head: TokenStream2,
        arms: Vec<(TokenStream2, Vec<Part>)>,
    },
    Let(TokenStream2),
}

struct Generator {
    parts: Vec<Part>,
    diagnostics: Vec<Diagnostic>,
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

    fn push_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        match self.parts.last_mut() {
            Some(Part::Static(last)) => last.push_str(s),
            _ => self.parts.push(Part::Static(s.to_owned())),
        }
    }

    fn push_escaped(&mut self, s: &str) {
        self.push_str(&escape(s));
    }

    fn push_render(&mut self, expr: TokenStream2, span: Span, owned: bool) {
        self.parts.push(Part::Render { expr, span, owned });
    }

    fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::spanned(span, Level::Error, msg.into()));
    }

    /// Whitespace between siblings follows JSX: a gap within one line
    /// (`{a} {b}`) is one space, a gap containing a line break is dropped.
    ///
    /// `open` and `close` are the surrounding tags, so that spaces between a
    /// tag and the text next to it are kept. Without them, text starts at its
    /// first word and ends at its last.
    fn nodes(&mut self, nodes: &[Node], ctx: Context, open: Option<Span>, close: Option<Span>) {
        // `@let` writes nothing, so the nodes on either side are neighbours.
        let is_let = |node: &Node| matches!(node, Node::Custom(ControlFlow::Let(_)));
        let mut prev: Option<&Node> = None;
        for (i, node) in nodes.iter().enumerate() {
            if is_let(node) {
                self.node(node, ctx, None, None);
                continue;
            }
            if ctx == Context::Normal
                && let Some(prev) = prev
                && same_line_gap(prev, node)
            {
                self.push_str(" ");
            }
            let before = prev.map_or(open, node_end);
            let next = nodes[i + 1..].iter().find(|n| !is_let(n));
            let after = next.map_or(close, node_start);
            self.node(node, ctx, before, after);
            prev = Some(node);
        }
    }

    fn node(&mut self, node: &Node, ctx: Context, before: Option<Span>, after: Option<Span>) {
        match node {
            Node::Element(el) => self.element(el),
            Node::Fragment(frag) => {
                let open = frag.tag_open.token_gt.span;
                let close = frag.tag_close.as_ref().map(|t| t.start_tag.token_lt.span);
                self.nodes(&frag.children, ctx, Some(open), close);
            }
            // rstml calls a quoted string `Text` and unquoted text `RawText`.
            Node::Text(text) => {
                let value = text.value_string();
                match ctx {
                    Context::Normal => self.push_escaped(&value),
                    Context::RawText(element) => self.raw_text_code(&value, text.span(), element),
                }
            }
            Node::RawText(raw) => match ctx {
                Context::Normal => {
                    let text = source_text(raw.to_token_stream(), before, after);
                    self.reject_nested_blocks(raw.to_token_stream());
                    self.push_escaped(&collapse_line_breaks(&text));
                }
                Context::RawText(element) => {
                    self.raw_text(raw.to_token_stream(), element, before, after);
                }
            },
            Node::Block(block) => {
                if let Some(tokens) = block_markup(block) {
                    let nodes = self.parse_markup(tokens);
                    self.nodes(&nodes, ctx, None, None);
                    return;
                }
                let owned = self.is_async && !block_is_place(block);
                self.push_render(block_tokens(block), block.span(), owned);
            }
            Node::Comment(comment) => self.error(
                comment.span(),
                "HTML comments aren't supported; write a Rust comment (`// ..`) instead",
            ),
            Node::Doctype(doctype) => {
                self.push_str("<!DOCTYPE ");
                self.push_str(&source_text(doctype.value.to_token_stream(), None, None));
                self.push_str(">");
            }
            Node::Custom(control_flow) => self.control_flow(control_flow),
        }
    }

    fn control_flow(&mut self, control_flow: &ControlFlow) {
        let part = match control_flow {
            ControlFlow::If(f) => {
                let branches = f.branches.iter().map(|branch| {
                    let else_token = &branch.else_token;
                    let cond = branch
                        .cond
                        .as_ref()
                        .map(|(if_token, cond)| quote!(#if_token #cond));
                    (quote!(#else_token #cond), self.body(&branch.body))
                });
                Part::Branches(branches.collect())
            }
            ControlFlow::For(f) => {
                let (for_token, pat, in_token, expr) = (&f.for_token, &f.pat, &f.in_token, &f.expr);
                let head = quote!(#for_token #pat #in_token #expr);
                Part::Branches(vec![(head, self.body(&f.body))])
            }
            ControlFlow::Match(f) => {
                let (match_token, expr) = (&f.match_token, &f.expr);
                let arms = f.arms.iter().map(|arm| {
                    let (pat, arrow) = (&arm.pat, &arm.arrow);
                    let guard = arm.guard.as_ref().map(|(if_token, g)| quote!(#if_token #g));
                    (quote!(#pat #guard #arrow), self.body(&arm.body))
                });
                Part::Match {
                    head: quote!(#match_token #expr),
                    arms: arms.collect(),
                }
            }
            ControlFlow::Let(f) => Part::Let(f.stmt.to_token_stream()),
            ControlFlow::Invalid(_) => return,
        };
        self.parts.push(part);
    }

    /// Walks a body with an empty part list swapped in, so its parts are
    /// returned instead of appended to the surrounding ones.
    fn body(&mut self, body: &Body) -> Vec<Part> {
        let outer = std::mem::take(&mut self.parts);
        self.nodes(&body.nodes, Context::Normal, None, None);
        std::mem::replace(&mut self.parts, outer)
    }

    /// rstml keeps `(..)` and `[..]` in unquoted text as single tokens, so a
    /// `{..}` inside them would be written out as text, not evaluated.
    fn reject_nested_blocks(&mut self, tokens: TokenStream2) {
        use proc_macro2::{Delimiter, TokenTree};
        for tt in tokens {
            let TokenTree::Group(group) = tt else {
                continue;
            };
            if group.delimiter() != Delimiter::Brace {
                self.reject_nested_blocks(group.stream());
                continue;
            }
            self.error(
                group.span(),
                "`{..}` inside `(..)` or `[..]` in text isn't rendered; \
                 quote the brackets: `\"(\"{value}\")\"`",
            );
        }
    }

    fn element(&mut self, el: &NodeElement) {
        // As in JSX, a tag whose name starts with an uppercase letter is a
        // component.
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

        let name = el.name().to_string();
        let lower = name.to_ascii_lowercase();
        let is_void = VOID_ELEMENTS.contains(&lower.as_str());

        if let NodeName::Block(block) = el.name() {
            self.error(block.span(), "dynamic tag names are not supported");
            return;
        }

        // rstml treats only lowercase `script` and `style` as raw text, but
        // browsers accept any case. Inside `<sCRIPT>`, values would be
        // written as HTML text, without the checks that code requires.
        if RAW_TEXT_ELEMENTS.contains(&lower.as_str()) && name != lower {
            self.error(el.name().span(), format!("write `<{lower}>` in lowercase"));
            return;
        }

        self.push_str("<");
        self.push_str(&name);
        for attr in el.attributes() {
            self.attribute(attr, &lower);
        }
        self.push_str(">");

        if is_void {
            if !el.children.is_empty() {
                self.error(
                    el.name().span(),
                    format!("<{name}> is a void element and cannot have children"),
                );
            }
            return;
        }

        let ctx = match lower.as_str() {
            "script" => Context::RawText(RawTextElement::Script),
            "style" => Context::RawText(RawTextElement::Style),
            _ => Context::Normal,
        };
        let (open, close) = tag_spans(el);
        self.nodes(&el.children, ctx, open, close);

        self.push_str("</");
        self.push_str(&name);
        self.push_str(">");
    }

    fn parse_markup(&mut self, tokens: &TokenStream2) -> Vec<Node> {
        let (nodes, diagnostics) = Parser::new(parser_config())
            .parse_recoverable(tokens.clone())
            .split_vec();
        self.diagnostics.extend(diagnostics);
        nodes
    }

    /// Builds `nodes` into a separate `Markup` value with its own buffer.
    /// Children and markup props are arguments of a function call, so they
    /// cannot write into the surrounding buffer.
    fn markup_value(
        &mut self,
        nodes: &[Node],
        open: Option<Span>,
        close: Option<Span>,
    ) -> TokenStream2 {
        let mut child = Generator::new(self.is_async);
        child.nodes(nodes, Context::Normal, open, close);
        self.diagnostics.append(&mut child.diagnostics);
        let body = child.finish();
        quote!({ #body })
    }

    /// The content of `<script>` or `<style>` is a string literal of code, a
    /// single `{value}`, or code written directly in the macro, whose braces
    /// belong to the JavaScript or CSS.
    fn raw_text(
        &mut self,
        tokens: TokenStream2,
        element: RawTextElement,
        before: Option<Span>,
        after: Option<Span>,
    ) {
        let span = tokens.span();
        let Some(mut pieces) = raw_text_pieces(tokens.clone()) else {
            let text = source_text(tokens, before, after);
            self.raw_text_code(&text, span, element);
            return;
        };
        if pieces.len() > 1 {
            let msg = match element {
                RawTextElement::Script => {
                    "write code with values in it as one `{js!(\"..{value}..\")}`"
                }
                RawTextElement::Style => "write the CSS as one string or one `{value}`",
            };
            self.error(span, msg);
            return;
        }
        match pieces.pop() {
            Some(RawTextPiece::Code(lit)) => self.raw_text_code(&lit.value(), lit.span(), element),
            Some(RawTextPiece::Value(block)) => {
                let expr = match block.stmts.as_slice() {
                    [syn::Stmt::Expr(inner, None)] => inner.to_token_stream(),
                    _ => block.to_token_stream(),
                };
                self.parts.push(Part::RawTextValue {
                    expr,
                    span: block.span(),
                    element,
                });
            }
            None => {}
        }
    }

    /// A browser ends the element at the first `</script` or `</style`, in
    /// any case, even inside a string. Inside `<script>`, a `<script` after
    /// a `<!--` makes the browser skip the next `</script>`.
    fn raw_text_code(&mut self, code: &str, span: Span, element: RawTextElement) {
        let lower = code.to_ascii_lowercase();
        let tag = element.tag();
        if lower.contains(&format!("</{tag}")) {
            let lang = match element {
                RawTextElement::Script => "JavaScript",
                RawTextElement::Style => "CSS",
            };
            self.error(
                span,
                format!(
                    "`</{tag}` would end the `<{tag}>` element here; \
                     inside a {lang} string, write `<\\/{tag}`"
                ),
            );
            return;
        }
        if element == RawTextElement::Script && lower.contains("<script") {
            self.error(
                span,
                "`<script` inside `<script>` can stop the element from ending; \
                 inside a JavaScript string, write `\\x3Cscript`",
            );
            return;
        }
        self.push_str(code);
    }

    /// `<Card title="x" featured>kids</Card>` becomes
    /// `Card(props_builder(&Card).title("x").featured(true).children(..).build())`,
    /// using the builder generated by `#[component]`.
    fn component(&mut self, el: &NodeElement, path: &syn::ExprPath) {
        let tag_span = path.span();
        let mut setters = Vec::new();

        for attr in el.attributes() {
            let attr = match attr {
                NodeAttribute::Attribute(a) => a,
                NodeAttribute::Block(b) => {
                    match shorthand_name(b) {
                        Some(name) => setters.push(quote_spanned!(name.span()=> .#name(#name))),
                        None => self.error(
                            b.span(),
                            "`{..}` on a component must be a single name; \
                             `{title}` is shorthand for `title={title}`",
                        ),
                    }
                    continue;
                }
            };
            // A prop named after a keyword (`type`) calls the raw identifier
            // setter (`.r#type(..)`).
            let key_span = attr.key.span();
            let key_str = attr.key.to_string();
            let key = match &attr.key {
                NodeName::Path(p) if p.path.get_ident().is_some() => {
                    let id = p.path.get_ident().unwrap();
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
            let value = match &attr.possible_value {
                KeyedAttributeValue::None => quote_spanned!(key_span=> true),
                KeyedAttributeValue::Binding(b) => {
                    self.error(b.span(), "closure bindings (`attr(x)`) are not supported");
                    continue;
                }
                KeyedAttributeValue::Value(_) => {
                    let Some(expr) = attr.value() else { continue };
                    match expr_markup(expr) {
                        Some(tokens) => {
                            let nodes = self.parse_markup(tokens);
                            self.markup_value(&nodes, None, None)
                        }
                        None => unbrace(expr),
                    }
                }
            };
            setters.push(quote_spanned!(key_span=> .#key(#value)));
        }

        if !el.children.is_empty() {
            let (open, close) = tag_spans(el);
            let body = self.markup_value(&el.children, open, close);
            setters.push(quote_spanned!(tag_span=> .children(#body)));
        }

        // Each mention of a generic function is a separate instantiation.
        // Binding the component once makes `props_builder` and the call share
        // one, so its generic parameters are inferred from the props.
        // `Span::mixed_site()` hides the binding from the user's code.
        let c = syn::Ident::new("__htmloxide_component", Span::mixed_site());
        let expr = quote_spanned!(tag_span=> {
            let #c = &#path;
            #c(::htmloxide::__private::props_builder(#c) #( #setters )* .build())
        });
        let owned = self.is_async;
        self.push_render(expr, tag_span, owned);
    }

    fn attribute(&mut self, attr: &NodeAttribute, element: &str) {
        match attr {
            NodeAttribute::Block(block) => match shorthand_name(block) {
                Some(name) => {
                    let key = name.unraw().to_string();
                    self.parts.push(Part::Attr {
                        ctx: attr_context(element, &key),
                        key,
                        expr: name.to_token_stream(),
                        span: name.span(),
                    });
                }
                None => self.error(
                    block.span(),
                    "`{..}` on an HTML tag must be a single name; \
                     `{disabled}` is shorthand for `disabled={disabled}`",
                ),
            },
            NodeAttribute::Attribute(attr) => self.keyed_attribute(attr, element),
        }
    }

    fn keyed_attribute(&mut self, attr: &KeyedAttribute, element: &str) {
        let key = attr.key.to_string();

        match &attr.possible_value {
            KeyedAttributeValue::None => {
                self.push_str(" ");
                self.push_str(&key);
            }
            KeyedAttributeValue::Binding(b) => {
                self.error(b.span(), "closure bindings (`attr(x)`) are not supported");
            }
            KeyedAttributeValue::Value(_) => {
                let Some(expr) = attr.value() else {
                    return;
                };
                if expr_markup(expr).is_some() {
                    self.error(expr.span(), MARKUP_IN_HTML_ATTRIBUTE);
                    return;
                }
                match literal(expr) {
                    Some(Literal::Bool(value)) if takes_true_false(&key) => {
                        self.push_str(" ");
                        self.push_str(&key);
                        self.push_str(if value { "=\"true\"" } else { "=\"false\"" });
                    }
                    Some(Literal::Bool(true)) => {
                        self.push_str(" ");
                        self.push_str(&key);
                    }
                    Some(Literal::Bool(false)) => {}
                    Some(Literal::Text(text)) => {
                        self.push_str(" ");
                        self.push_str(&key);
                        self.push_str("=\"");
                        self.push_escaped(&text);
                        self.push_str("\"");
                    }
                    None => {
                        self.parts.push(Part::Attr {
                            ctx: attr_context(element, &key),
                            key,
                            expr: unbrace(expr),
                            span: expr.span(),
                        });
                    }
                }
            }
        }
    }

    fn finish(self) -> TokenStream2 {
        let static_len: usize = self
            .parts
            .iter()
            .map(|p| if let Part::Static(s) = p { s.len() } else { 0 })
            .sum();

        // `Span::mixed_site()` makes the name hygienic: code inside `{..}`
        // cannot refer to it, and a user variable of the same name does not
        // clash with it.
        let buf = syn::Ident::new("__htmloxide_buf", Span::mixed_site());
        let stmts = statements(self.parts, &buf);

        quote!(
            let mut #buf = ::std::string::String::with_capacity(#static_len);
            #stmts
            ::htmloxide::PreEscaped(#buf)
        )
    }
}

/// A procedural macro sees tokens, not types, so it cannot tell whether a
/// value is a `bool`, an `Option`, `Markup` or a future. Each value is handed
/// to a macro or function in `htmloxide::__private`, where the way to write
/// it is selected from its type during type checking.
fn statements(parts: Vec<Part>, buf: &syn::Ident) -> TokenStream2 {
    let body = |(head, parts): (TokenStream2, Vec<Part>)| {
        let stmts = statements(parts, buf);
        quote!(#head { #stmts })
    };
    parts
        .into_iter()
        .map(|part| match part {
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
            Part::Attr {
                key,
                expr,
                span,
                ctx,
            } => {
                let key = format!(" {key}");
                match ctx {
                    AttrContext::Text => quote_spanned!(span=>
                        ::htmloxide::__private::attr!(#key, &(#expr), &mut #buf, Text);
                    ),
                    AttrContext::Words => quote_spanned!(span=>
                        ::htmloxide::__private::attr!(#key, &(#expr), &mut #buf, Words);
                    ),
                    AttrContext::Url => quote_spanned!(span=>
                        ::htmloxide::__private::attr!(#key, &(#expr), &mut #buf, Url);
                    ),
                    AttrContext::Srcdoc => quote_spanned!(span=>
                        ::htmloxide::__private::attr!(#key, &(#expr), &mut #buf, Srcdoc);
                    ),
                    AttrContext::Json => quote_spanned!(span=>
                        ::htmloxide::__private::attr_json!(#key, &(#expr), &mut #buf);
                    ),
                    AttrContext::Js => quote_spanned!(span=>
                        ::htmloxide::__private::attr_js(#key, &(#expr), &mut #buf);
                    ),
                    AttrContext::StaticUrl => quote_spanned!(span=>
                        ::htmloxide::__private::attr_static(#key, &(#expr), &mut #buf);
                    ),
                }
            }
            Part::RawTextValue {
                expr,
                span,
                element: RawTextElement::Script,
            } => quote_spanned!(span=>
                ::htmloxide::__private::script_value(&(#expr), &mut #buf);
            ),
            Part::RawTextValue {
                expr,
                span,
                element: RawTextElement::Style,
            } => quote_spanned!(span=>
                ::htmloxide::__private::style_value(&(#expr), &mut #buf);
            ),
            Part::Branches(branches) => branches.into_iter().map(body).collect(),
            Part::Match { head, arms } => {
                let arms = arms.into_iter().map(body);
                quote!(#head { #( #arms )* })
            }
            Part::Let(stmt) => stmt,
        })
        .collect()
}

const MARKUP_IN_HTML_ATTRIBUTE: &str =
    "markup can only be passed to component props; HTML attribute values are text";

fn parser_config() -> ParserConfig<ControlFlow> {
    ParserConfig::new()
        // Keeps a block that is not valid Rust yet (`{user.}` while typing)
        // as a node, so rust-analyzer can still complete inside it.
        .recover_block(true)
        .always_self_closed_elements(HashSet::from(VOID_ELEMENTS))
        .raw_text_elements(HashSet::from(RAW_TEXT_ELEMENTS))
        .transform_block(mark_markup)
        .custom_node::<ControlFlow>()
}

/// rstml parses the inside of every `{..}` as Rust, but a prop can take
/// markup: `<Card title={<b>Hi</b>}>`. This hook runs on each block first.
/// When the inside is markup, it is replaced with
/// `::htmloxide::__private::markup! { <b>Hi</b> }`, which is valid Rust and
/// carries the tokens unchanged. The generator detects that call
/// (`marked_markup`) and parses its tokens as markup; the call itself never
/// reaches the output.
///
/// Rust can also start with `<` (`<T as Trait>::f()`), so the inside counts
/// as markup only when it does not parse as Rust.
fn mark_markup(input: ParseStream) -> syn::Result<Option<TokenStream2>> {
    let starts_like_markup = input
        .cursor()
        .punct()
        .is_some_and(|(p, _)| matches!(p.as_char(), '<' | '@'));
    if !starts_like_markup || input.fork().call(syn::Block::parse_within).is_ok() {
        return Ok(None);
    }
    // Consumed one token at a time: `input.parse::<TokenStream2>()` leaves
    // the stream at a position from which rstml panics.
    let mut tokens = TokenStream2::new();
    while !input.is_empty() {
        tokens.extend([input.parse::<proc_macro2::TokenTree>()?]);
    }
    Ok(Some(quote!(::htmloxide::__private::markup! { #tokens })))
}

fn marked_markup(stmts: &[syn::Stmt]) -> Option<&TokenStream2> {
    let [syn::Stmt::Macro(m)] = stmts else {
        return None;
    };
    let path = &m.mac.path;
    let is_wrapper = path.leading_colon.is_some()
        && path.segments.iter().map(|s| s.ident.to_string()).eq([
            "htmloxide",
            "__private",
            "markup",
        ]);
    is_wrapper.then_some(&m.mac.tokens)
}

fn expr_markup(expr: &Expr) -> Option<&TokenStream2> {
    match expr {
        Expr::Block(b) => marked_markup(&b.block.stmts),
        _ => None,
    }
}

fn block_markup(block: &NodeBlock) -> Option<&TokenStream2> {
    match block {
        NodeBlock::ValidBlock(b) => marked_markup(&b.stmts),
        NodeBlock::Invalid(_) => None,
    }
}

enum RawTextPiece {
    Code(syn::LitStr),
    Value(syn::Block),
}

fn raw_text_pieces(tokens: TokenStream2) -> Option<Vec<RawTextPiece>> {
    use proc_macro2::{Delimiter, TokenTree};
    tokens
        .into_iter()
        .map(|tt| match &tt {
            TokenTree::Literal(_) => syn::parse2(tt.into_token_stream())
                .ok()
                .map(RawTextPiece::Code),
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => {
                syn::parse2(tt.into_token_stream())
                    .ok()
                    .map(RawTextPiece::Value)
            }
            _ => None,
        })
        .collect()
}

/// `{x}` -> `x`, for the same reason as `block_tokens`, and so that a
/// setter argument does not trip the `unused_braces` lint. syn parses
/// `{html! { .. }}` as a macro statement rather than an expression, hence the
/// second case.
fn unbrace(expr: &Expr) -> TokenStream2 {
    let Expr::Block(b) = expr else {
        return expr.to_token_stream();
    };
    if !b.attrs.is_empty() || b.label.is_some() {
        return expr.to_token_stream();
    }

    match b.block.stmts.as_slice() {
        [syn::Stmt::Expr(inner, None)] => inner.to_token_stream(),
        [syn::Stmt::Macro(m)] if m.attrs.is_empty() && m.semi_token.is_none() => {
            m.mac.to_token_stream()
        }
        _ => expr.to_token_stream(),
    }
}

fn shorthand_name(block: &NodeBlock) -> Option<&syn::Ident> {
    let NodeBlock::ValidBlock(b) = block else {
        return None;
    };
    let [syn::Stmt::Expr(Expr::Path(p), None)] = b.stmts.as_slice() else {
        return None;
    };
    if !p.attrs.is_empty() || p.qself.is_some() {
        return None;
    }
    p.path.get_ident()
}

enum Literal {
    Text(String),
    Bool(bool),
}

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

/// Whether a `bool` is written as `key="true"` / `key="false"` instead of
/// adding or omitting the attribute.
fn takes_true_false(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    let hx = key
        .strip_prefix("data-")
        .filter(|k| k.starts_with("hx-"))
        .or_else(|| key.starts_with("hx-").then_some(key.as_str()));
    match hx {
        Some(hx) => HTMX_TRUE_FALSE_ATTRIBUTES.contains(&hx),
        None => {
            key.starts_with("aria-")
                || key.starts_with("data-")
                || TRUE_FALSE_ATTRIBUTES.contains(&key.as_str())
        }
    }
}

/// `{name}` is emitted as `name`. A block expression moves its value, so
/// `&({name})` would move `name` into a temporary and leave it unusable
/// after the markup.
fn block_tokens(block: &NodeBlock) -> TokenStream2 {
    match block {
        NodeBlock::ValidBlock(b) => match b.stmts.as_slice() {
            [syn::Stmt::Expr(inner, None)] => inner.to_token_stream(),
            _ => b.to_token_stream(),
        },
        NodeBlock::Invalid(_) => block.to_token_stream(),
    }
}

/// JSX rule for unquoted text: each line is trimmed at its line breaks,
/// blank lines are dropped, and the rest are joined with one space.
fn collapse_line_breaks(text: &str) -> String {
    if !text.contains('\n') {
        return text.to_owned();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len() - 1;
    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.into_iter().enumerate() {
        let mut line = line.strip_suffix('\r').unwrap_or(line);
        if i > 0 {
            line = line.trim_start_matches([' ', '\t']);
        }
        if i < last {
            line = line.trim_end_matches([' ', '\t']);
        }
        if line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
    }
    out
}

/// The compiler passes tokens, not text, and tokens carry no whitespace:
/// `Hello world` arrives as `Hello` and `world`. Each token's span holds its
/// line and column, so the text is rebuilt from the gaps between spans. The
/// gaps to `before` and `after`, where the neighbouring nodes end and start,
/// are included, so `Hi {name}` keeps its space.
///
/// Some tools report every span as 0:0, and tokens produced by another macro
/// can share one position. The tokens are then printed as `TokenStream`
/// prints them.
fn source_text(tokens: TokenStream2, before: Option<Span>, after: Option<Span>) -> String {
    let mut pieces = Vec::new();
    text_pieces(tokens.clone(), &mut pieces);
    let in_order = pieces.windows(2).all(|w| w[0].end <= w[1].start);
    let located = pieces.iter().all(|p| p.start.line > 0 && p.start < p.end);
    let (Some(first), Some(last)) = (pieces.first(), pieces.last()) else {
        return String::new();
    };
    if !in_order || !located {
        return tokens.to_string();
    }

    let mut out = String::new();
    if let Some(before) = before
        .map(|s| s.end())
        .filter(|b| b.line > 0 && *b <= first.start)
    {
        push_gap(&mut out, before, first.start);
    }
    let mut prev_end = first.start;
    for piece in &pieces {
        push_gap(&mut out, prev_end, piece.start);
        out.push_str(&piece.text);
        prev_end = piece.end;
    }
    if let Some(after) = after
        .map(|s| s.start())
        .filter(|a| a.line > 0 && last.end <= *a)
    {
        push_gap(&mut out, last.end, after);
    }
    out
}

struct TextPiece {
    text: String,
    start: LineColumn,
    end: LineColumn,
}

fn text_pieces(tokens: TokenStream2, out: &mut Vec<TextPiece>) {
    use proc_macro2::{Delimiter, TokenTree};
    for tt in tokens {
        let TokenTree::Group(group) = tt else {
            let span = tt.span();
            out.push(TextPiece {
                text: tt.to_string(),
                start: span.start(),
                end: span.end(),
            });
            continue;
        };
        let (open, close) = match group.delimiter() {
            Delimiter::Parenthesis => ("(", ")"),
            Delimiter::Bracket => ("[", "]"),
            Delimiter::Brace => ("{", "}"),
            // An invisible group, such as a `$x` substituted by
            // `macro_rules!`.
            Delimiter::None => {
                text_pieces(group.stream(), out);
                continue;
            }
        };
        let (span_open, span_close) = (group.span_open(), group.span_close());
        out.push(TextPiece {
            text: open.to_owned(),
            start: span_open.start(),
            end: span_open.end(),
        });
        text_pieces(group.stream(), out);
        out.push(TextPiece {
            text: close.to_owned(),
            start: span_close.start(),
            end: span_close.end(),
        });
    }
}

fn push_gap(out: &mut String, from: LineColumn, to: LineColumn) {
    if to.line > from.line {
        out.push('\n');
    } else {
        out.extend(std::iter::repeat_n(
            ' ',
            to.column.saturating_sub(from.column),
        ));
    }
}

fn same_line_gap(prev: &Node, next: &Node) -> bool {
    if matches!(prev, Node::RawText(_)) || matches!(next, Node::RawText(_)) {
        return false;
    }
    let (Some(last), Some(first)) = (node_end(prev), node_start(next)) else {
        return false;
    };
    let (end, start) = (last.end(), first.start());
    end.line != 0 && end.line == start.line && start.column > end.column
}

fn node_start(node: &Node) -> Option<Span> {
    node.to_token_stream()
        .into_iter()
        .next()
        .map(|tt| tt.span())
}

fn node_end(node: &Node) -> Option<Span> {
    use proc_macro2::TokenTree;
    node.to_token_stream()
        .into_iter()
        .last()
        .map(|tt| match tt {
            TokenTree::Group(g) => g.span_close(),
            tt => tt.span(),
        })
}

fn tag_spans(el: &NodeElement) -> (Option<Span>, Option<Span>) {
    let open = el.open_tag.end_tag.token_gt.span;
    let close = el.close_tag.as_ref().map(|t| t.start_tag.token_lt.span);
    (Some(open), close)
}

/// A place expression (`x`, `x.y`, `x[i]`, `*x`) names an existing value.
/// `html_async!` borrows it, as `html!` does, and moves every other value so
/// that a future can be awaited.
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
        NodeBlock::Invalid(_) => true,
    }
}

/// The characters maud escapes in values at runtime.
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
