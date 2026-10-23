//! Tests for `#[component]`: required, default, optional and `into` props,
//! children, generics, lifetimes, components in other modules, and calling a
//! component as a plain function.

use std::borrow::Cow;

use htmloxide::{Markup, Render, component, each, html};

// --------------------------------------------------
// Component: Badge
// --------------------------------------------------

// The smallest component: one required prop, no children.
#[component]
fn Badge(count: u32) -> Markup {
    html! { <span class="badge">{count}</span> }
}

#[test]
fn simple_prop() {
    assert_eq!(
        html! { <Badge count=3 /> }.into_string(),
        r#"<span class="badge">3</span>"#
    );
}

// --------------------------------------------------
// Component: Card
// --------------------------------------------------

// One prop of each kind:
// - `title`: required.
// - `subtitle`: optional, and `into`, so the tag passes a `&str` and the
//   function gets an `Option<String>`.
// - `featured`: `false` when left out.
// - `children`: whatever is between `<Card>` and `</Card>`.
#[component]
fn Card(
    title: &str,
    #[prop(optional, into)] subtitle: Option<String>,
    #[prop(default)] featured: bool,
    children: Markup,
) -> Markup {
    html! {
        <section class="card" data-featured={featured}>
            <h2>{title}</h2>
            {subtitle.map(|s| html! { <h3>{s}</h3> })}
            {children}
        </section>
    }
}

// First every prop is given (`featured` with no value means `true`), then
// only the required ones: no `<h3>` and no `data-featured`.
#[test]
fn children_defaults_optional_and_into() {
    let name = String::from("Ada");
    let out = html! {
        <Card title={&name} subtitle="intro" featured>
            <p>Hello {name}</p>
        </Card>
    };
    assert_eq!(
        out.into_string(),
        r#"<section class="card" data-featured><h2>Ada</h2><h3>intro</h3><p>Hello Ada</p></section>"#
    );

    let out = html! { <Card title="Bare"><i>x</i></Card> };
    assert_eq!(
        out.into_string(),
        r#"<section class="card"><h2>Bare</h2><i>x</i></section>"#
    );
}

// --------------------------------------------------
// Component: Layout
// --------------------------------------------------

// `level` has a default expression instead of `Default::default()`.
#[component]
fn Layout(#[prop(into)] title: String, #[prop(default = 1)] level: u8, children: Markup) -> Markup {
    html! {
        <!DOCTYPE html>
        <html>
            <head><title>{&title}</title></head>
            <body data-level={level}>{children}</body>
        </html>
    }
}

// Components nested in a component's children, and `level` left at its
// default of 1.
#[test]
fn nested_components_and_default_expr() {
    let out = html! {
        <Layout title="Home">
            <Card title="One"><Badge count=1 /></Card>
            <Card title="Two"><Badge count={1 + 1} /></Card>
        </Layout>
    };
    assert_eq!(
        out.into_string(),
        "<!DOCTYPE html><html><head><title>Home</title></head><body data-level=\"1\">\
         <section class=\"card\"><h2>One</h2><span class=\"badge\">1</span></section>\
         <section class=\"card\"><h2>Two</h2><span class=\"badge\">2</span></section>\
         </body></html>"
    );
}

// --------------------------------------------------
// Component: List
// --------------------------------------------------

// A generic component: `T` is inferred from the `items` given in the tag.
// `_kind` adds a defaulted prop that doesn't involve `T`.
#[component]
fn List<T: Render>(items: Vec<T>, #[prop(default = "ul")] _kind: &'static str) -> Markup {
    html! { <ul>{each(items.iter().map(|i| html! { <li>{i}</li> }))}</ul> }
}

#[test]
fn generic_component() {
    let out = html! { <List items={vec![1, 2, 3]} /> };
    assert_eq!(out.into_string(), "<ul><li>1</li><li>2</li><li>3</li></ul>");
}

// --------------------------------------------------
// Component: Label
// --------------------------------------------------

// A named lifetime (`'a`) next to an elided one (`&str`, which becomes
// `'props`), and a prop named after a Rust keyword: `r#for` in the function,
// `for` in the tag.
#[component]
fn Label<'a>(text: Cow<'a, str>, r#for: &str) -> Markup {
    html! { <label for={r#for}>{text}</label> }
}

#[test]
fn explicit_lifetimes_and_keyword_props() {
    let out = html! { <Label text={Cow::Borrowed("Name")} for="name" /> };
    assert_eq!(out.into_string(), r#"<label for="name">Name</label>"#);
}

// --------------------------------------------------
// Component: ui::Button
// --------------------------------------------------

// A component in another module. The tag names it by path, and its props
// struct (`ButtonProps`) is never imported.
mod ui {
    use htmloxide::{Markup, component, html};

    #[component]
    pub fn Button(#[prop(into)] label: String, #[prop(default)] disabled: bool) -> Markup {
        html! { <button disabled={disabled}>{label}</button> }
    }
}

#[test]
fn path_components_without_importing_props() {
    let out = html! { <div><ui::Button label="Go" /><ui::Button label="No" disabled /></div> };
    assert_eq!(
        out.into_string(),
        "<div><button>Go</button><button disabled>No</button></div>"
    );
}

// --------------------------------------------------
// Component: Footer
// --------------------------------------------------

// No props at all: `<Footer />` calls `build()` on an empty builder.
#[component]
fn Footer() -> Markup {
    html! { <footer>"(c) 2026"</footer> }
}

#[test]
fn no_props() {
    assert_eq!(
        html! { <Footer /> }.into_string(),
        "<footer>(c) 2026</footer>"
    );
}

// --------------------------------------------------
// Tests: other ways to use a component
// --------------------------------------------------

// Outside `html!`, a component is a function taking its props struct.
#[test]
fn still_callable_as_function() {
    let out = Badge(BadgeProps { count: 9 });
    assert_eq!(out.into_string(), r#"<span class="badge">9</span>"#);
}

// A component tag inside an `each(..)` closure, with a prop from the loop.
#[test]
fn components_in_loops() {
    let counts = [1u32, 2];
    let out = html! { <div>{each(counts.iter().map(|&c| html! { <Badge count={c} /> }))}</div> };
    assert_eq!(
        out.into_string(),
        r#"<div><span class="badge">1</span><span class="badge">2</span></div>"#
    );
}
