//! `html!` with plain HTML: text, escaping, tags, attributes, whitespace,
//! and how values of each type are written.

use htmloxide::{Js, Markup, PreEscaped, Render, each, html, js};

fn s(m: Markup) -> String {
    m.into_string()
}

#[test]
fn elements_and_static_attributes() {
    let out = html! { <div class="card" id="main"><p>"quoted text"</p></div> };
    assert_eq!(
        s(out),
        r#"<div class="card" id="main"><p>quoted text</p></div>"#
    );
}

#[test]
fn unquoted_text_keeps_spaces() {
    let out = html! { <p>Hello world, how are you?</p> };
    assert_eq!(s(out), "<p>Hello world, how are you?</p>");
}

#[test]
fn mixed_text_and_blocks() {
    let name = "Ada";
    let n = 3;
    let out = html! { <p>Hi {name}, you have {n} new messages.</p> };
    assert_eq!(s(out), "<p>Hi Ada, you have 3 new messages.</p>");
}

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

#[test]
fn static_literals_are_escaped_at_compile_time() {
    let out = html! { <p title="a \"b\" <c>">"1 < 2 & 3 > 2"</p> };
    assert_eq!(
        s(out),
        r#"<p title="a &quot;b&quot; &lt;c&gt;">1 &lt; 2 &amp; 3 &gt; 2</p>"#
    );
}

#[test]
fn pre_escaped_is_not_escaped() {
    let raw = PreEscaped("<b>bold</b>");
    let out = html! { <div>{raw}</div> };
    assert_eq!(s(out), "<div><b>bold</b></div>");
}

// The attribute holds the markup's HTML as text, so the escaped text inside
// the markup is escaped a second time.
#[test]
fn markup_in_attributes() {
    let user = "<script>";
    let tip = html! { <b class="x">{user}</b> };
    let out = html! { <span data-tip={tip}></span> };
    assert_eq!(
        s(out),
        r#"<span data-tip="&lt;b class=&quot;x&quot;&gt;&amp;lt;script&amp;gt;&lt;/b&gt;"></span>"#
    );
}

#[test]
fn render_impl_quotes_in_attributes() {
    struct Raw;
    impl Render for Raw {
        fn render_to(&self, b: &mut String) {
            b.push_str(r#"x" onclick="alert(1)"#);
        }
    }
    let out = html! { <p title={Raw}></p> };
    assert_eq!(s(out), r#"<p title="x&quot; onclick=&quot;alert(1)"></p>"#);
}

// Browsers do not treat `<span/>` as closed, so it gets a closing tag.
#[test]
fn void_elements_and_self_closing() {
    let out = html! { <div><br><img src="a.png" alt="A"><input type="text" /><span /></div> };
    assert_eq!(
        s(out),
        r#"<div><br><img src="a.png" alt="A"><input type="text"><span></span></div>"#
    );
}

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

#[test]
fn dashed_and_namespaced_attributes() {
    let out = html! { <button hx-post="/clicked" data-id="7" aria-label="Go" hx-on:click="x()">Go</button> };
    assert_eq!(
        s(out),
        r#"<button hx-post="/clicked" data-id="7" aria-label="Go" hx-on:click="x()">Go</button>"#
    );
}

#[test]
fn custom_elements() {
    let out = html! { <my-widget size="2"></my-widget> };
    assert_eq!(s(out), r#"<my-widget size="2"></my-widget>"#);
}

#[test]
fn boolean_attributes() {
    let on = true;
    let off = false;
    let out = html! { <input disabled checked=true hidden=false readonly={on} required={off}> };
    assert_eq!(s(out), "<input disabled checked readonly>");
}

#[test]
fn true_false_attributes() {
    let on = true;
    let off = false;
    let out = html! {
        <b aria-pressed={on} aria-expanded={off} aria-hidden=true draggable={off}
           data-open={on} data-closed={off} data-fixed=true></b>
    };
    assert_eq!(
        s(out),
        concat!(
            r#"<b aria-pressed="true" aria-expanded="false" aria-hidden="true" draggable="false""#,
            r#" data-open="true" data-closed="false" data-fixed="true"></b>"#,
        )
    );
}

#[test]
fn optional_bool_attributes() {
    let none: Option<bool> = None;
    let yes = Some(true);
    let no = Some(false);
    let out = html! {
        <input aria-invalid={none} aria-busy={yes} aria-hidden={no} disabled={none} checked={yes} required={no}>
    };
    assert_eq!(
        s(out),
        r#"<input aria-busy="true" aria-hidden="false" checked>"#
    );
}

// `data-hx-*` follows the htmx rule, not the `data-*` one.
#[test]
fn htmx_bool_attributes() {
    let on = true;
    let off = false;
    let out = html! {
        <div hx-boost={on} hx-push-url={off} hx-history={off} hx-preserve={on} data-hx-boost={on}></div>
        <div hx-preserve={off} data-hx-preserve={off}></div>
    };
    assert_eq!(
        s(out),
        concat!(
            r#"<div hx-boost="true" hx-push-url="false" hx-history="false" hx-preserve data-hx-boost="true"></div>"#,
            "<div></div>",
        )
    );
}

#[test]
fn optional_attributes() {
    let some: Option<&str> = Some("x");
    let none: Option<String> = None;
    let out = html! { <a href={some} title={none}>link</a> };
    assert_eq!(s(out), r#"<a href="x">link</a>"#);
}

#[test]
fn shorthand_attributes() {
    let r#type = "text";
    let disabled = true;
    let hidden: Option<&str> = None;
    let title = "x onmouseover=alert(1)";
    let out = html! { <input {r#type} {disabled} {hidden} {title}> };
    assert_eq!(
        s(out),
        r#"<input type="text" disabled title="x onmouseover=alert(1)">"#
    );
}

#[test]
fn js_attributes() {
    let id = 7;
    let name = "\"Ada\" & co";
    let none: Option<Js> = None;
    let out = html! {
        <button onclick={js!("select({id}, {name})")} data-hx-on:click={js!("a && b")} onfocus={none} onblur="save()">x</button>
    };
    assert_eq!(
        s(out),
        concat!(
            r#"<button onclick="select(7, &quot;\&quot;Ada\&quot; \u0026 co&quot;)""#,
            r#" data-hx-on:click="a &amp;&amp; b" onblur="save()">x</button>"#,
        )
    );
}

// A literal `javascript:` URL is part of the source, so it is not blocked.
#[test]
fn url_attributes() {
    let bad = " \tJaVa\nScRiPt:alert(1)";
    let vb = "vbscript:x";
    let ok = "/about?a=1&b=2";
    let out = html! {
        <a href={bad}>a</a><img src={vb}><a href={ok}>b</a>
        <a href={js!("history.back()")}>c</a><a href="javascript:void(0)">d</a>
    };
    assert_eq!(
        s(out),
        concat!(
            r#"<a href="about:invalid#blocked">a</a><img src="about:invalid#blocked">"#,
            r#"<a href="/about?a=1&amp;b=2">b</a><a href="javascript:history.back()">c</a>"#,
            r#"<a href="javascript:void(0)">d</a>"#,
        )
    );
}

#[test]
fn static_urls() {
    const APP: &str = "/static/app.js";
    let analytics: Option<&'static str> = None;
    let out =
        html! { <base href={APP}><script src={APP}></script><script src={analytics}></script> };
    assert_eq!(
        s(out),
        r#"<base href="/static/app.js"><script src="/static/app.js"></script><script></script>"#
    );
}

#[test]
fn htmx_json_attributes() {
    use std::collections::BTreeMap;
    let text = r#"{"id": 1}"#;
    let map = BTreeMap::from([("id", 1)]);
    let out = html! {
        <div hx-vals={text}></div><div hx-vals={map}></div>
        <div hx-headers={js!("token: getToken()")}></div>
    };
    assert_eq!(
        s(out),
        concat!(
            r#"<div hx-vals="{&quot;id&quot;: 1}"></div><div hx-vals="{&quot;id&quot;:1}"></div>"#,
            r#"<div hx-headers="js:token: getToken()"></div>"#,
        )
    );
}

#[test]
fn srcdoc() {
    let user = "<script>alert(1)</script>";
    let doc = html! { <p>{user}</p> };
    let out = html! { <iframe srcdoc={doc}></iframe><iframe srcdoc={user}></iframe> };
    assert_eq!(
        s(out),
        concat!(
            r#"<iframe srcdoc="&lt;p&gt;&amp;lt;script&amp;gt;alert(1)&amp;lt;/script&amp;gt;&lt;/p&gt;"></iframe>"#,
            r#"<iframe srcdoc="&amp;lt;script&amp;gt;alert(1)&amp;lt;/script&amp;gt;"></iframe>"#,
        )
    );
}

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

// `Ipv4Addr` implements `Display` but not `Render`.
#[test]
fn display_fallback() {
    let ip = std::net::Ipv4Addr::LOCALHOST;
    let out = html! { <code data-ip={ip}>{ip}</code> };
    assert_eq!(s(out), r#"<code data-ip="127.0.0.1">127.0.0.1</code>"#);
}

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

#[test]
fn interop_with_maud() {
    let from_maud = maud::html! { b { "maud" } };
    let out = html! { <p>{from_maud}</p> };
    let back = maud::html! { div { (out) } };
    assert_eq!(s(back), "<div><p><b>maud</b></p></div>");
}

#[test]
fn fragments_and_comments() {
    let out = html! {
        <>
            // a comment
            <b>a</b><i>b</i>
        </>
    };
    assert_eq!(s(out), "<b>a</b><i>b</i>");
}

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

// Unquoted text is rebuilt from token positions, which must be usable
// however the macro is called.
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

#[test]
fn nested_macro_text() {
    let out = html! { <ul>{each((1..=2).map(|i| html! { <li>Item number {i}</li> }))}</ul> };
    assert_eq!(
        s(out),
        "<ul><li>Item number 1</li><li>Item number 2</li></ul>"
    );
}

#[test]
fn blocks_borrow_instead_of_moving() {
    let name = String::from("Ada");
    let class = String::from("big");
    let out = html! { <p class={class}>{name}</p> };
    assert_eq!(name.len() + class.len(), 6);
    assert_eq!(s(out), r#"<p class="big">Ada</p>"#);
}

#[test]
fn markup_inside_blocks() {
    let out = html! { <p>{<b>Hi</b>} {<>there</>}</p> };
    assert_eq!(s(out), "<p><b>Hi</b> there</p>");
}

#[test]
fn script_values_are_data() {
    let code = "alert(1)";
    assert_eq!(
        s(html! { <script>{code}</script> }),
        r#"<script>"alert(1)"</script>"#
    );

    let name = "</script><script>alert(1)</script>";
    assert_eq!(
        s(html! { <script>{name}</script> }),
        r#"<script>"\u003c/script\u003e\u003cscript\u003ealert(1)\u003c/script\u003e"</script>"#
    );

    let tricky = "\"\\\n\u{2028}&";
    assert_eq!(
        s(html! { <script>{tricky}</script> }),
        r#"<script>"\"\\\n\u2028\u0026"</script>"#
    );
}

// JSON cannot hold NaN, so it is written as `null`.
#[test]
fn js_template_values() {
    use std::collections::BTreeMap;
    let n = 3;
    let x = 1.5;
    let nan = f64::NAN;
    let yes = true;
    let c = 'c';
    let none: Option<u8> = None;
    let list = vec!["a", "<b>"];
    let map = BTreeMap::from([("k", 1)]);
    let out = html! {
        <script>{js!("f({n},{x},{nan},{yes},{c},{none},{list},{map});")}</script>
    };
    assert_eq!(
        s(out),
        r#"<script>f(3,1.5,null,true,"c",null,["a","\u003cb\u003e"],{"k":1});</script>"#
    );
}

#[test]
fn js_template_arguments() {
    let inner = js!("go()");
    let code = js!("if ({}) {{ {inner}; {n} }}", 1 + 1, n = "x");
    assert_eq!(code.as_str(), r#"if (2) { go(); "x" }"#);
}

#[test]
fn code_cant_end_its_element() {
    const JS: &str = "x = '</script><SCRIPT>'; y = a < b;";
    let css = "a::after { content: '</STYLE>'; } p > a { color: red }";
    let out = html! { <script>{Js::from_static(JS)}</script><style>{css}</style> };
    assert_eq!(
        s(out),
        concat!(
            r"<script>x = '</\u0073cript><\u0053CRIPT>'; y = a < b;</script>",
            r"<style>a::after { content: '</\53 TYLE>'; } p > a { color: red }</style>",
        )
    );
}

#[test]
fn json_island() {
    let todos = ["one", "two"];
    let out = html! { <script type="application/json" id="todos">{todos}</script> };
    assert_eq!(
        s(out),
        r#"<script type="application/json" id="todos">["one","two"]</script>"#
    );
}

#[test]
fn whitespace_between_siblings_follows_jsx() {
    let (a, b) = (1, "x");
    let out = html! { <p>{a} {b}</p> };
    assert_eq!(s(out), "<p>1 x</p>");
    let out = html! { <p><b>bold</b> {b} <i>it</i></p> };
    assert_eq!(s(out), "<p><b>bold</b> x <i>it</i></p>");
    let out = html! { <p>{a}{b}</p> };
    assert_eq!(s(out), "<p>1x</p>");
    let out = html! {
        <ul>
            <li>a</li>
            <li>b</li>
        </ul>
    };
    assert_eq!(s(out), "<ul><li>a</li><li>b</li></ul>");
}

#[test]
fn line_breaks_in_text_follow_jsx() {
    let n = 2;
    let out = html! {
        <p>
            Hello
            world
        </p>
    };
    assert_eq!(s(out), "<p>Hello world</p>");
    let out = html! {
        <li>
            <a>x</a>
            {n} items
        </li>
    };
    assert_eq!(s(out), "<li><a>x</a>2 items</li>");
    let out = html! { <pre>"a\n  b"</pre> };
    assert_eq!(s(out), "<pre>a\n  b</pre>");
}
