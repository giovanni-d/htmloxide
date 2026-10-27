//! `serde::Serialize` values written as JSON: inside `<script>`, in `js!`
//! and in `hx-vals`.

use htmloxide::{html, js};
use serde::Serialize;

#[derive(Serialize)]
struct User {
    name: String,
    admin: bool,
    tags: Vec<&'static str>,
}

#[test]
fn serialize_values_in_script() {
    let user = User {
        name: "</script>&\u{2028}".into(),
        admin: true,
        tags: vec!["a"],
    };
    let out = html! { <script>{js!("window.user = {user};")}</script> };
    assert_eq!(
        out.into_string(),
        r#"<script>window.user = {"name":"\u003c/script\u003e\u0026\u2028","admin":true,"tags":["a"]};</script>"#
    );
}

#[test]
fn serialize_values_in_hx_vals() {
    #[derive(Serialize)]
    struct Vals {
        id: u32,
    }
    let out = html! { <button hx-post="/x" hx-vals={Vals { id: 7 }}>x</button> };
    assert_eq!(
        out.into_string(),
        r#"<button hx-post="/x" hx-vals="{&quot;id&quot;:7}">x</button>"#
    );
}
