//! Tests for `html!` with plain HTML: text, escaping, tags, attributes,
//! whitespace, and how values of different types render. Each test compares
//! the generated HTML with the exact string a browser would receive.

use htmloxide::{Markup, PreEscaped, Render, each, html};

/// Shorthand for `into_string()`, to keep the assertions short.
fn s(m: Markup) -> String {
    m.into_string()
}

// --------------------------------------------------
// Tests: elements and text
// --------------------------------------------------

// Quoted text is written without its quotes.
#[test]
fn elements_and_static_attributes() {
    let out = html! { <div class="card" id="main"><p>"quoted text"</p></div> };
    assert_eq!(
        s(out),
        r#"<div class="card" id="main"><p>quoted text</p></div>"#
    );
}

// Rust tokens don't record spaces, so this checks that the macro recovers
// them from the source text.
#[test]
fn unquoted_text_keeps_spaces() {
    let out = html! { <p>Hello world, how are you?</p> };
    assert_eq!(s(out), "<p>Hello world, how are you?</p>");
}

// Unquoted text keeps the spaces next to `{..}` blocks, and punctuation
// right after a block stays attached to it.
#[test]
fn mixed_text_and_blocks() {
    let name = "Ada";
    let n = 3;
    let out = html! { <p>Hi {name}, you have {n} new messages.</p> };
    assert_eq!(s(out), "<p>Hi Ada, you have 3 new messages.</p>");
}

// --------------------------------------------------
// Tests: escaping
// --------------------------------------------------

// A value from `{..}` can't inject markup, in an attribute or in text.
#[test]
fn dynamic_values_are_escaped() {
    let evil = r#"<script>alert("x")</script>"#;
    let out = html! { <p title={evil}>{evil}</p> };
    assert_eq!(
        s(out),
        "<p title=\"&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt;\">\
         &lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt;</p>"
    );
}

// Literals get the same escaping as `{..}` values, applied by the macro.
#[test]
fn static_literals_are_escaped_at_compile_time() {
    let out = html! { <p title="a \"b\" <c>">"1 < 2 & 3 > 2"</p> };
    assert_eq!(
        s(out),
        r#"<p title="a &quot;b&quot; &lt;c&gt;">1 &lt; 2 &amp; 3 &gt; 2</p>"#
    );
}

// `PreEscaped` opts out of escaping, for HTML that is already safe.
#[test]
fn pre_escaped_is_not_escaped() {
    let raw = PreEscaped("<b>bold</b>");
    let out = html! { <div>{raw}</div> };
    assert_eq!(s(out), "<div><b>bold</b></div>");
}

// --------------------------------------------------
// Tests: HTML syntax
// --------------------------------------------------

// Void elements (`<br>`, `<img>`, `<input>`) need no closing tag, and
// `<input />` is written as `<input>`. `<span />` gets a real closing tag,
// because browsers don't treat `<span/>` as closed.
#[test]
fn void_elements_and_self_closing() {
    let out = html! { <div><br><img src="a.png" alt="A"><input type="text" /><span /></div> };
    assert_eq!(
        s(out),
        r#"<div><br><img src="a.png" alt="A"><input type="text"><span></span></div>"#
    );
}

// The line breaks between tags are formatting and don't reach the output.
#[test]
fn doctype_and_full_page() {
    let title = "Home";
    let out = html! {
        <!DOCTYPE html>
        <html lang="en">
            <head><title>{title}</title></head>
            <body></body>
        </html>
    };
    assert_eq!(
        s(out),
        r#"<!DOCTYPE html><html lang="en"><head><title>Home</title></head><body></body></html>"#
    );
}

// htmx (`hx-post`, `hx-on:click`), `data-*` and `aria-*` names are written as
// typed.
#[test]
fn dashed_and_namespaced_attributes() {
    let out = html! { <button hx-post="/clicked" data-id="7" aria-label="Go" hx-on:click="x()">Go</button> };
    assert_eq!(
        s(out),
        r#"<button hx-post="/clicked" data-id="7" aria-label="Go" hx-on:click="x()">Go</button>"#
    );
}

// Web components have dashed tag names.
#[test]
fn custom_elements() {
    let out = html! { <my-widget size="2"></my-widget> };
    assert_eq!(s(out), r#"<my-widget size="2"></my-widget>"#);
}

// --------------------------------------------------
// Tests: attribute values
// --------------------------------------------------

// `true` keeps the attribute and `false` drops it, whether the value is a
// literal (handled at compile time) or a `bool` variable (at runtime).
#[test]
fn boolean_attributes() {
    let on = true;
    let off = false;
    let out = html! { <input disabled checked=true hidden=false readonly={on} required={off}> };
    assert_eq!(s(out), "<input disabled checked readonly>");
}

// `Some(x)` writes the attribute with `x` as its value; `None` drops it.
#[test]
fn optional_attributes() {
    let some: Option<&str> = Some("x");
    let none: Option<String> = None;
    let out = html! { <a href={some} title={none}>link</a> };
    assert_eq!(s(out), r#"<a href="x">link</a>"#);
}

// Numbers and arithmetic in `{..}` render at runtime; the literal
// `tabindex=3` becomes `"3"` at compile time.
#[test]
fn numeric_and_expression_attributes() {
    let w = 640u32;
    let ratio = 1.5f64;
    let out = html! { <img width={w} height={w / 2} data-ratio={ratio} tabindex=3> };
    assert_eq!(
        s(out),
        r#"<img width="640" height="320" data-ratio="1.5" tabindex="3">"#
    );
}

// `Ipv4Addr` implements `Display` but not `Render`, so it renders through
// `Display`, both as an attribute and as text.
#[test]
fn display_fallback() {
    let ip = std::net::Ipv4Addr::LOCALHOST;
    let out = html! { <code data-ip={ip}>{ip}</code> };
    assert_eq!(s(out), r#"<code data-ip="127.0.0.1">127.0.0.1</code>"#);
}

// --------------------------------------------------
// Tests: Rust inside the markup
// --------------------------------------------------

// A collected `Vec<Markup>` renders each item. A lazy iterator has to go
// through `each`. Both give the same, escaped, result.
#[test]
fn loops_with_collections_and_each() {
    let items = ["one", "<two>"];
    let lis: Vec<Markup> = items.iter().map(|i| html! { <li>{i}</li> }).collect();
    let a = html! { <ul>{lis}</ul> };
    let b = html! { <ul>{each(items.iter().map(|i| html! { <li>{i}</li> }))}</ul> };
    let expected = "<ul><li>one</li><li>&lt;two&gt;</li></ul>";
    assert_eq!(s(a), expected);
    assert_eq!(s(b), expected);
}

// `if` / `else` and `bool::then` are plain Rust. `then` gives an `Option`,
// and `None` renders nothing.
#[test]
fn conditionals() {
    let logged_in = true;
    let admin = false;
    let out = html! {
        <nav>
            {if logged_in { html! { <a href="/logout">Log out</a> } } else { html! { <a href="/login">Log in</a> } }}
            {admin.then(|| html! { <a href="/admin">Admin</a> })}
        </nav>
    };
    assert_eq!(s(out), r#"<nav><a href="/logout">Log out</a></nav>"#);
}

// --------------------------------------------------
// Tests: reusing markup
// --------------------------------------------------

// Without `#[component]`, a function returning `Markup` is called inside
// `{..}` like any other expression.
#[test]
fn components_are_just_functions() {
    fn card(title: &str, body: Markup) -> Markup {
        html! { <section class="card"><h2>{title}</h2>{body}</section> }
    }
    let out = html! { <main>{card("Hi", html! { <p>body</p> })}</main> };
    assert_eq!(
        s(out),
        r#"<main><section class="card"><h2>Hi</h2><p>body</p></section></main>"#
    );
}

// Any type implementing `Render` can go inside `{..}`.
#[test]
fn custom_render_impl() {
    struct Badge(u32);
    impl Render for Badge {
        fn render(&self) -> Markup {
            html! { <span class="badge">{self.0}</span> }
        }
    }
    let out = html! { <div>{Badge(5)}</div> };
    assert_eq!(s(out), r#"<div><span class="badge">5</span></div>"#);
}

// `Markup` is maud's type, so it passes between maud's `html!` and ours in
// both directions.
#[test]
fn interop_with_maud() {
    let from_maud = maud::html! { b { "maud" } };
    let out = html! { <p>{from_maud}</p> };
    let back = maud::html! { div { (out) } };
    assert_eq!(s(back), "<div><p><b>maud</b></p></div>");
}

// --------------------------------------------------
// Tests: fragments, comments, raw text and spreads
// --------------------------------------------------

// `<>..</>` adds no tag of its own; `<!-- "..." -->` becomes an HTML comment.
#[test]
fn fragments_and_comments() {
    let out = html! { <><!-- "a comment" --><b>a</b><i>b</i></> };
    assert_eq!(s(out), "<!-- a comment --><b>a</b><i>b</i>");
}

// CSS and JS in a string literal go out as written: `>`, `<`, `&&` and the
// quotes are left alone.
#[test]
fn script_and_style_are_not_escaped() {
    let out = html! {
        <style>"p > a { color: red; }"</style>
        <script>"if (a < b && c) { go(\"x\"); }"</script>
    };
    assert_eq!(
        s(out),
        r#"<style>p > a { color: red; }</style><script>if (a < b && c) { go("x"); }</script>"#
    );
}

// A `{..}` block where an attribute would go is written into the tag as is.
#[test]
fn spread_attribute_block() {
    let extra = "data-x";
    let out = html! { <div {extra}></div> };
    assert_eq!(s(out), "<div data-x></div>");
}

// --------------------------------------------------
// Tests: calling the macro
// --------------------------------------------------
//
// To keep the spaces in unquoted text, the macro finds its own call in the
// source file. These tests check that this works however the call is
// written.

#[test]
fn called_with_path() {
    let out = htmloxide::html! { <p>Path call works</p> };
    assert_eq!(s(out), "<p>Path call works</p>");
}

#[test]
fn called_with_parens() {
    let out = html!(<p>Paren call works</p>);
    assert_eq!(s(out), "<p>Paren call works</p>");
}

// An `html!` inside another `html!`.
#[test]
fn nested_macro_text() {
    let out = html! { <ul>{each((1..=2).map(|i| html! { <li>Item number {i}</li> }))}</ul> };
    assert_eq!(
        s(out),
        "<ul><li>Item number 1</li><li>Item number 2</li></ul>"
    );
}

// --------------------------------------------------
// Tests: {..} blocks
// --------------------------------------------------

#[test]
fn blocks_borrow_instead_of_moving() {
    let name = String::from("Ada");
    let class = String::from("big");
    let out = html! { <p class={class}>{name}</p> };
    // both still usable afterwards
    assert_eq!(name.len() + class.len(), 6);
    assert_eq!(s(out), r#"<p class="big">Ada</p>"#);
}

// A lone `{..}` in `<style>` / `<script>` still goes through `Render`.
// Plain strings are escaped, so data can't close the `<script>` early;
// trusted CSS or JS is passed in `PreEscaped`.
#[test]
fn blocks_inside_style_and_script() {
    const CSS: &str = "a > b { color: red }";
    let data = "</script><b>";
    let out = html! { <style>{PreEscaped(CSS)}</style><script>{data}</script> };
    assert_eq!(
        s(out),
        "<style>a > b { color: red }</style><script>&lt;/script&gt;&lt;b&gt;</script>"
    );
}

// --------------------------------------------------
// Tests: whitespace
// --------------------------------------------------

// Between two siblings that aren't text, a space on the same line is kept as
// one space, and no space stays no space (the JSX rule).
#[test]
fn whitespace_between_siblings_follows_jsx() {
    let (a, b) = (1, "x");
    let out = html! { <p>{a} {b}</p> };
    assert_eq!(s(out), "<p>1 x</p>");
    let out = html! { <p><b>bold</b> {b} <i>it</i></p> };
    assert_eq!(s(out), "<p><b>bold</b> x <i>it</i></p>");
    let out = html! { <p>{a}{b}</p> };
    assert_eq!(s(out), "<p>1x</p>");
    // Newlines between siblings are formatting, not content.
    let out = html! {
        <ul>
            <li>a</li>
            <li>b</li>
        </ul>
    };
    assert_eq!(s(out), "<ul><li>a</li><li>b</li></ul>");
}
