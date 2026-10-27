//! `#[component]`: required, default, optional and `into` props, children,
//! generics, lifetimes, and components in other modules.

use std::borrow::Cow;

use htmloxide::{Markup, Render, component, html};

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

#[component]
fn Card(
    title: &str,
    #[prop(into)] subtitle: Option<String>,
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
        r#"<section class="card" data-featured="true"><h2>Ada</h2><h3>intro</h3><p>Hello Ada</p></section>"#
    );

    let out = html! { <Card title="Bare"><i>x</i></Card> };
    assert_eq!(
        out.into_string(),
        r#"<section class="card" data-featured="false"><h2>Bare</h2><i>x</i></section>"#
    );
}

#[component]
fn Layout(#[prop(into)] title: String, #[prop(default = 1)] level: u8, children: Markup) -> Markup {
    html! {
        <!DOCTYPE html>
        <html>
            <head><title>{title}</title></head>
            <body data-level={level}>{children}</body>
        </html>
    }
}

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
         <section class=\"card\" data-featured=\"false\"><h2>One</h2><span class=\"badge\">1</span></section>\
         <section class=\"card\" data-featured=\"false\"><h2>Two</h2><span class=\"badge\">2</span></section>\
         </body></html>"
    );
}

// `_kind`'s setter must not declare `T`, which it could not infer.
#[component]
fn List<T: Render>(items: Vec<T>, #[prop(default = "ul")] _kind: &'static str) -> Markup {
    html! { <ul>@for i in &items { <li>{i}</li> }</ul> }
}

#[test]
fn generic_component() {
    let out = html! { <List items={vec![1, 2, 3]} /> };
    assert_eq!(out.into_string(), "<ul><li>1</li><li>2</li><li>3</li></ul>");
}

#[component]
fn Label<'a>(text: Cow<'a, str>, r#for: &str) -> Markup {
    html! { <label for={r#for}>{text}</label> }
}

#[test]
fn explicit_lifetimes_and_keyword_props() {
    let out = html! { <Label text={Cow::Borrowed("Name")} for="name" /> };
    assert_eq!(out.into_string(), r#"<label for="name">Name</label>"#);
}

#[test]
fn shorthand_props() {
    let title = "Hi";
    let featured = true;
    let out = html! { <Card {title} subtitle="intro" {featured}>x</Card> };
    assert_eq!(
        out.into_string(),
        r#"<section class="card" data-featured="true"><h2>Hi</h2><h3>intro</h3>x</section>"#
    );

    let text = Cow::Borrowed("Name");
    let r#for = "name";
    let out = html! { <Label {text} {r#for} /> };
    assert_eq!(out.into_string(), r#"<label for="name">Name</label>"#);
}

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

#[component]
fn Titled<T: Render>(title: T, children: Markup) -> Markup {
    html! { <section><h2>{title}</h2>{children}</section> }
}

// The braces around `html! { .. }` must not reach the setter call.
#[test]
#[deny(unused_braces)]
fn macro_prop_value() {
    let out = html! { <Titled title={html! { <b>Hi</b> }}><p>Body</p></Titled> };
    assert_eq!(
        out.into_string(),
        "<section><h2><b>Hi</b></h2><p>Body</p></section>"
    );
}

#[component]
fn Profile(
    nickname: Option<String>,
    #[prop(into)] bio: Option<String>,
    #[prop(default = Some("/default.png"))] avatar: Option<&str>,
) -> Markup {
    html! { <p data-nick={nickname} title={bio}><img src={avatar}></p> }
}

#[test]
fn option_props() {
    let out = html! { <Profile /> };
    assert_eq!(out.into_string(), r#"<p><img src="/default.png"></p>"#);

    let out = html! { <Profile nickname={String::from("ada")} bio="hi" avatar="/a.png" /> };
    assert_eq!(
        out.into_string(),
        r#"<p data-nick="ada" title="hi"><img src="/a.png"></p>"#
    );

    let out =
        html! { <Profile nickname={Some(String::from("ada"))} bio={Some("hi")} avatar={None} /> };
    assert_eq!(
        out.into_string(),
        r#"<p data-nick="ada" title="hi"><img></p>"#
    );

    let nickname: Option<String> = None;
    let bio: Option<&str> = Some("hi");
    let out = html! { <Profile nickname={nickname} bio={bio} /> };
    assert_eq!(
        out.into_string(),
        r#"<p title="hi"><img src="/default.png"></p>"#
    );
}

#[component]
fn Panel(children: Option<Markup>) -> Markup {
    html! { <div>@if let Some(c) = children { {c} } else { "empty" }</div> }
}

#[test]
fn optional_children() {
    assert_eq!(html! { <Panel /> }.into_string(), "<div>empty</div>");
    assert_eq!(
        html! { <Panel><b>x</b></Panel> }.into_string(),
        "<div><b>x</b></div>"
    );
}

// The builder's type parameters are named after the props. Here they would
// shadow the types `char` and `u8`.
#[component]
fn Glyph(char: char, #[prop(default = u8::MAX)] u8: u8) -> Markup {
    html! { <i data-max={u8}>{char}</i> }
}

#[test]
fn props_named_like_types() {
    assert_eq!(
        html! { <Glyph char='a' /> }.into_string(),
        r#"<i data-max="255">a</i>"#
    );
}

#[component]
fn Dialog(title: Markup, #[prop(default)] children: Markup) -> Markup {
    html! { <dialog><h2>{title}</h2>{children}</dialog> }
}

#[test]
fn markup_props() {
    let name = "<Ada>";
    let admin = true;
    let out = html! {
        <Dialog title={<b>Hi {name}</b>}><p>Body</p></Dialog>
        <Dialog title={<>Hi <b>there</b></>} />
        <Dialog title={@if admin { <b>Admin</b> } else { User }} />
        <Dialog title={ <i>a</i> <i>b</i> } />
        <Dialog title={<Dialog title={<Badge count=2 />} />} />
    };
    assert_eq!(
        out.into_string(),
        concat!(
            "<dialog><h2><b>Hi &lt;Ada&gt;</b></h2><p>Body</p></dialog>",
            "<dialog><h2>Hi <b>there</b></h2></dialog>",
            "<dialog><h2><b>Admin</b></h2></dialog>",
            "<dialog><h2><i>a</i> <i>b</i></h2></dialog>",
            r#"<dialog><h2><dialog><h2><span class="badge">2</span></h2></dialog></h2></dialog>"#,
        )
    );
}

#[test]
fn rust_starting_with_angle_bracket_stays_rust() {
    let out = html! { <Dialog title={<Markup as Default>::default()} /> };
    assert_eq!(out.into_string(), "<dialog><h2></h2></dialog>");
}

#[test]
fn still_callable_as_function() {
    let out = Badge(BadgeProps { count: 9 });
    assert_eq!(out.into_string(), r#"<span class="badge">9</span>"#);
}

#[test]
fn components_in_loops() {
    let counts = [1u32, 2];
    let out = html! { <div>@for count in counts { <Badge {count} /> }</div> };
    assert_eq!(
        out.into_string(),
        r#"<div><span class="badge">1</span><span class="badge">2</span></div>"#
    );
}
