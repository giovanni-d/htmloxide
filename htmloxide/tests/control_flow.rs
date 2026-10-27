//! `@if`, `@for`, `@match` and `@let`: each construct, nesting, components
//! in bodies, and whitespace around and inside bodies.

use htmloxide::{Markup, component, html};

fn s(m: Markup) -> String {
    m.into_string()
}

#[test]
fn if_else_chain() {
    let render = |n: i32| {
        s(html! {
            <p>
                @if n < 0 { negative } else if n == 0 { zero } else { <b>positive</b> }
            </p>
        })
    };
    assert_eq!(render(-1), "<p>negative</p>");
    assert_eq!(render(0), "<p>zero</p>");
    assert_eq!(render(5), "<p><b>positive</b></p>");
}

#[test]
fn if_let() {
    let render = |user: Option<&str>| {
        s(html! {
            @if let Some(name) = user {
                <p>Signed in as {name}.</p>
            } else {
                <a href="/login">Log in</a>
            }
        })
    };
    assert_eq!(render(Some("Ada")), "<p>Signed in as Ada.</p>");
    assert_eq!(render(None), r#"<a href="/login">Log in</a>"#);
}

#[test]
fn for_loop_with_let() {
    let items = [String::from("a"), String::from("b")];
    let out = html! {
        <ul>
            @for (i, item) in items.iter().enumerate() {
                @let number = i + 1;
                <li>{number}. {item}</li>
            }
        </ul>
    };
    assert_eq!(s(out), "<ul><li>1. a</li><li>2. b</li></ul>");
    assert_eq!(items.len(), 2);
}

#[test]
fn match_with_guards_and_alternatives() {
    let render = |n: u32| {
        s(html! {
            @match n {
                0 => { <p>none</p> }
                1 | 2 => { <p>a few</p> },
                n if n > 100 => { <p>lots</p> }
                n => { <p>{n}</p> }
            }
        })
    };
    assert_eq!(render(0), "<p>none</p>");
    assert_eq!(render(2), "<p>a few</p>");
    assert_eq!(render(500), "<p>lots</p>");
    assert_eq!(render(7), "<p>7</p>");
}

#[test]
fn nested_bodies() {
    let rows = [vec![1, 2], vec![]];
    let out = html! {
        @for row in &rows {
            <tr>
                @if row.is_empty() {
                    <td>"-"</td>
                } else {
                    @for cell in row { <td>{cell}</td> }
                }
            </tr>
        }
    };
    assert_eq!(s(out), "<tr><td>1</td><td>2</td></tr><tr><td>-</td></tr>");
}

#[component]
fn Badge(count: u32) -> Markup {
    html! { <span class="badge">{count}</span> }
}

#[test]
fn components_in_bodies() {
    let counts = [1, 2];
    let out = html! { <div>@for c in counts { <Badge count={c} /> }</div> };
    assert_eq!(
        s(out),
        r#"<div><span class="badge">1</span><span class="badge">2</span></div>"#
    );
}

#[test]
fn text_in_bodies() {
    let found = 0;
    let out = html! { <p>@if found == 0 { No posts found, sorry. }</p> };
    assert_eq!(s(out), "<p>No posts found, sorry.</p>");
}

#[test]
fn spacing_around_control_flow() {
    let required = true;
    let out = html! { <label>Name @if required { <b>*</b> }</label> };
    assert_eq!(s(out), "<label>Name <b>*</b></label>");
    let out = html! { <label>{"Name"}@if required { <b>*</b> }</label> };
    assert_eq!(s(out), "<label>Name<b>*</b></label>");
    let out = html! {
        <label>
            Name
            @if required { <b>*</b> }
        </label>
    };
    assert_eq!(s(out), "<label>Name<b>*</b></label>");
}

#[test]
fn let_adds_no_space() {
    let out = html! { <p>@let n = 2; {n} items</p> };
    assert_eq!(s(out), "<p>2 items</p>");
}

#[test]
fn text_from_macro_rules() {
    macro_rules! greeting {
        ($name:expr) => {
            html! { <p>Hello, {$name}! How are you?</p> }
        };
    }
    assert_eq!(s(greeting!("Ada")), "<p>Hello, Ada! How are you?</p>");
}
