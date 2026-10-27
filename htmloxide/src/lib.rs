//! **htmloxide**: write HTML in Rust that looks like HTML.
//!
//! ```
//! use htmloxide::html;
//!
//! let name = "<world>";
//! let page = html! {
//!     <div class="greeting">
//!         <h1>Hello, {name}!</h1>
//!     </div>
//! };
//! assert_eq!(
//!     page.into_string(),
//!     r#"<div class="greeting"><h1>Hello, &lt;world&gt;!</h1></div>"#
//! );
//! ```
//!
//! * [`html!`] builds [`Markup`] from HTML-like syntax; `{expr}` values are escaped.
//!   `@if`, `@for`, `@match` and `@let` write Rust control flow inside it.
//! * [`#[component]`](component) turns a function into a `<Tag prop=..>` with
//!   compile-time-checked props.
//! * [`html_async!`] does the same for async components and evaluates to a future.
//!
//! Anything implementing [`Render`] (or `Display`) can be used inside `{..}`.
//!
//! With the `axum` feature, [`Markup`] implements axum's `IntoResponse`, so a
//! handler can return it directly.

// A `proc-macro` crate can export nothing but macros, so the macros live in
// `htmloxide-macros` and the code they generate calls into `__private` here.

// The generated code names this crate `::htmloxide`, which also has to
// resolve inside the crate itself, for its own tests.
extern crate self as htmloxide;

use std::borrow::Cow;
use std::cell::Cell;

pub use htmloxide_macros::{component, html, html_async, js};
// maud's types, so that `Markup` and `Render` impls work across both crates.
pub use maud::{Escaper, Markup, PreEscaped, Render, display};

/// Render every item of an iterator in place.
///
/// Blocks borrow their value (`{expr}` is rendered from `&expr`), so a lazy
/// iterator such as `items.iter().map(..)` can't be consumed directly.
/// Collections (`Vec`, slices, arrays, `Option`) work without this; use
/// `each` for lazy iterator chains:
///
/// ```
/// use htmloxide::{each, html};
/// let items = ["a", "b"];
/// let list = html! { <ul>{each(items.iter().map(|i| html! { <li>{i}</li> }))}</ul> };
/// assert_eq!(list.into_string(), "<ul><li>a</li><li>b</li></ul>");
/// ```
///
/// The iterator is consumed on first render; rendering the same `Each`
/// twice yields nothing the second time.
pub fn each<I>(iter: I) -> Each<I>
where
    I: IntoIterator,
    I::Item: Render,
{
    Each(Cell::new(Some(iter)))
}

/// An iterator wrapped by [`each`] so it can be rendered inside `{..}`.
//
// `render_to` gets `&self`, but iterating needs ownership. `Cell::take` moves
// the iterator out through the shared reference.
pub struct Each<I>(Cell<Option<I>>);

/// Await a collection of futures concurrently and concatenate their markup
/// in order. Use inside `html_async!`, typically with async components:
///
/// ```ignore
/// html_async! {
///     <ul>{each_async(ids.iter().map(|&id| html_async! { <UserRow id={id} /> }))}</ul>
/// }
/// ```
pub async fn each_async<I>(futures: I) -> Markup
where
    I: IntoIterator,
    I::Item: std::future::Future,
    <I::Item as std::future::Future>::Output: Render,
{
    let mut out = String::new();
    for item in futures_util::future::join_all(futures).await {
        item.render_to(&mut out);
    }
    PreEscaped(out)
}

impl<I> Render for Each<I>
where
    I: IntoIterator,
    I::Item: Render,
{
    fn render_to(&self, buffer: &mut String) {
        if let Some(iter) = self.0.take() {
            for item in iter {
                item.render_to(buffer);
            }
        }
    }
}

/// JavaScript code, created by [`js!`] or [`Js::from_static`] so that it
/// always comes from the program's source.
///
/// Attributes that run code, such as `onclick`, accept only `Js`. Inside
/// `<script>` and in a `js!` template, a `Js` is written as code and any
/// other value as JSON.
#[derive(Clone, Debug)]
pub struct Js(Cow<'static, str>);

impl Js {
    /// Code compiled into the program, such as a file read with
    /// `include_str!`.
    ///
    /// ```
    /// use htmloxide::{Js, html};
    /// const APP: &str = "boot();";
    /// let page = html! { <script>{Js::from_static(APP)}</script> };
    /// assert_eq!(page.into_string(), "<script>boot();</script>");
    /// ```
    pub fn from_static(code: &'static str) -> Js {
        Js(Cow::Borrowed(code))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Runtime support for the code the macros generate. Not public API.
//
// The macros write fixed HTML as string literals and hand every `{..}` value
// to this module, which writes it into the buffer. How a value is written
// depends on its type: a `bool` attribute is present or absent, an `Option`
// may omit it, `Markup` is already HTML, anything else is escaped text. A
// procedural macro cannot see types, so the choice is made here, during type
// checking, by a technique called autoref specialization.
//
// Each way of writing a value is a tier: a zero-sized kind type whose
// `render` method does the writing, and a trait whose only method returns
// that kind. The method has the same name in every tier. Each trait is
// implemented for `Wrap<&T>` behind a different number of references, with
// the bounds that tier needs:
//
//     impl<T: Render>  ViaRender  for &&&Wrap<&T>  // tried first
//     impl<T: Display> ViaDisplay for    Wrap<&T>  // tried last
//
// The generated code calls the method on more references than any tier
// uses:
//
//     (&&&&Wrap(value)).htmloxide_node_kind().render(value, buf)
//
// To resolve a method call, the compiler tries the receiver type as written,
// then with one reference removed, then two, and uses the first type for
// which an impl exists and its bounds hold. So the tier with the most
// references wins whenever its bounds hold. Since the method takes `&self`,
// an impl for `&&&Wrap<&T>` is found at receiver `&&&&Wrap<&T>`: the call
// site needs one reference more than the first tier. The tiers are separate
// traits because stable Rust rejects overlapping impls of one trait.
//
// Two tiers at the same depth that both match a type are a compile error
// (E0034). A change to the order or bounds of the tiers changes how existing
// templates escape their values, without a compile error, and needs the same
// review as a change to the escaping code.
#[doc(hidden)]
pub mod __private {
    use std::fmt::Display;

    use maud::Render;

    // An impl on a bare `T` would match at several depths: `T: Display`
    // also holds for `&&&T`.
    pub struct Wrap<T>(pub T);

    pub struct Unset;
    pub struct Set<T>(pub T);

    pub trait Props {
        type Builder;
        fn builder() -> Self::Builder;
    }

    // Only the argument's type is used: `Fn(P) -> R` lets the compiler infer
    // the props struct from the component function.
    #[inline]
    pub fn props_builder<P: Props, R>(_component: &impl Fn(P) -> R) -> P::Builder {
        P::builder()
    }

    pub trait PropOrDefault<T> {
        fn take_or(self, default: impl FnOnce() -> T) -> T;
    }
    impl<T> PropOrDefault<T> for Set<T> {
        #[inline]
        fn take_or(self, _: impl FnOnce() -> T) -> T {
            self.0
        }
    }
    impl<T> PropOrDefault<T> for Unset {
        #[inline]
        fn take_or(self, default: impl FnOnce() -> T) -> T {
            default()
        }
    }

    // The setter of an `Option<T>` prop with `#[prop(into)]` accepts a value
    // that converts into `T`, or an `Option` of one. As two impls of one
    // trait they would overlap, since an `Option<U>` may itself convert into
    // `T`. The marker `K` makes them impls of two different traits, and the
    // compiler infers `K` from the one whose bounds the value meets.
    pub trait IntoOptionProp<T, K> {
        fn into_option_prop(self) -> Option<T>;
    }
    pub struct FromValue;
    pub struct FromOption;
    impl<T, U: Into<T>> IntoOptionProp<T, FromValue> for U {
        #[inline]
        fn into_option_prop(self) -> Option<T> {
            Some(self.into())
        }
    }
    impl<T, U: Into<T>> IntoOptionProp<T, FromOption> for Option<U> {
        #[inline]
        fn into_option_prop(self) -> Option<T> {
            self.map(Into::into)
        }
    }

    // `#[macro_export]` places a `macro_rules!` macro at the crate root; the
    // `pub use` below each one gives it the path the generated code uses.
    //
    // `match $value { value => .. }` evaluates the expression once and keeps
    // its temporaries alive until the value is written.

    #[macro_export]
    #[doc(hidden)]
    macro_rules! __htmloxide_render {
        ($value:expr, $buf:expr) => {
            match $value {
                value => {
                    #[allow(unused_imports)]
                    use $crate::__private::node::*;
                    (&&&&$crate::__private::Wrap(value))
                        .htmloxide_node_kind()
                        .render(value, $buf)
                }
            }
        };
    }
    pub use crate::__htmloxide_render as render;

    // Awaits the value if it is a future, then writes the result.
    #[macro_export]
    #[doc(hidden)]
    macro_rules! __htmloxide_render_async {
        ($value:expr, $buf:expr) => {
            match $crate::__private::Wrap($value) {
                wrapped => {
                    let resolved = {
                        #[allow(unused_imports)]
                        use $crate::__private::resolve::*;
                        (&&wrapped).htmloxide_resolve_kind().resolve(wrapped.0)
                    }
                    .await;
                    $crate::__private::render!(&resolved, $buf)
                }
            }
        };
    }
    pub use crate::__htmloxide_render_async as render_async;

    // A value that is not a future is wrapped in `ready`, so the macro can
    // await either kind.
    pub mod resolve {
        use super::*;
        use std::future::{IntoFuture, Ready, ready};

        pub struct FutureKind;
        pub struct ReadyKind;

        pub trait ViaFuture {
            fn htmloxide_resolve_kind(&self) -> FutureKind {
                FutureKind
            }
        }
        pub trait ViaReady {
            fn htmloxide_resolve_kind(&self) -> ReadyKind {
                ReadyKind
            }
        }
        impl<T: IntoFuture> ViaFuture for &Wrap<T> {}
        impl<T> ViaReady for Wrap<T> {}

        impl FutureKind {
            #[inline]
            pub fn resolve<T: IntoFuture>(self, value: T) -> T::IntoFuture {
                value.into_future()
            }
        }
        impl ReadyKind {
            #[inline]
            pub fn resolve<T>(self, value: T) -> Ready<T> {
                ready(value)
            }
        }
    }

    #[macro_export]
    #[doc(hidden)]
    macro_rules! __htmloxide_attr {
        ($key:expr, $value:expr, $buf:expr, $ctx:ident) => {
            match $value {
                value => {
                    #[allow(unused_imports)]
                    use $crate::__private::attr::*;
                    (&&&&&&&$crate::__private::Wrap(value))
                        .htmloxide_attr_kind()
                        .render($key, value, $buf, $crate::__private::attr::Ctx::$ctx)
                }
            }
        };
    }
    pub use crate::__htmloxide_attr as attr;

    // `attr!` with one more tier, below `Display`: any `serde::Serialize`
    // value, written as JSON.
    #[macro_export]
    #[doc(hidden)]
    macro_rules! __htmloxide_attr_json {
        ($key:expr, $value:expr, $buf:expr) => {
            match $value {
                value => {
                    #[allow(unused_imports)]
                    use $crate::__private::attr::*;
                    #[allow(unused_imports)]
                    use $crate::__private::attr_json::*;
                    (&&&&&&&$crate::__private::Wrap(value))
                        .htmloxide_attr_kind()
                        .render($key, value, $buf, $crate::__private::attr::Ctx::Json)
                }
            }
        };
    }
    pub use crate::__htmloxide_attr_json as attr_json;

    pub mod node {
        use super::*;

        pub struct RenderKind;
        pub struct IterKind;
        pub struct FutureKind;
        pub struct DisplayKind;

        pub trait ViaRender {
            fn htmloxide_node_kind(&self) -> RenderKind {
                RenderKind
            }
        }
        pub trait ViaIter {
            fn htmloxide_node_kind(&self) -> IterKind {
                IterKind
            }
        }
        pub trait ViaFuture {
            fn htmloxide_node_kind(&self) -> FutureKind {
                FutureKind
            }
        }
        pub trait ViaDisplay {
            fn htmloxide_node_kind(&self) -> DisplayKind {
                DisplayKind
            }
        }

        #[diagnostic::on_unimplemented(
            message = "async component or future used inside `html!`",
            label = "this is a future",
            note = "use `html_async! {{ .. }}.await` to render async components"
        )]
        pub trait UseHtmlAsync {}

        // Order: `Render`, iterable of `Render`, future, `Display`. Futures
        // come before `Display` so that a future gets the error about
        // `html_async!` rather than "doesn't implement Display".
        impl<T: Render + ?Sized> ViaRender for &&&Wrap<&T> {}
        impl<T: std::future::Future + ?Sized> ViaFuture for &Wrap<&T> {}
        impl<'a, T: ?Sized> ViaIter for &&Wrap<&'a T>
        where
            &'a T: IntoIterator,
            <&'a T as IntoIterator>::Item: Render,
        {
        }
        impl<T: Display + ?Sized> ViaDisplay for Wrap<&T> {}

        impl RenderKind {
            #[inline]
            pub fn render<T: Render + ?Sized>(self, value: &T, buf: &mut String) {
                value.render_to(buf);
            }
        }
        impl IterKind {
            #[inline]
            pub fn render<'a, T: ?Sized>(self, value: &'a T, buf: &mut String)
            where
                &'a T: IntoIterator,
                <&'a T as IntoIterator>::Item: Render,
            {
                for item in value {
                    item.render_to(buf);
                }
            }
        }
        // No type implements `UseHtmlAsync`, so a call never compiles.
        impl FutureKind {
            pub fn render<T: ?Sized + UseHtmlAsync>(self, _: &T, _: &mut String) {
                unreachable!()
            }
        }
        impl DisplayKind {
            #[inline]
            pub fn render<T: Display + ?Sized>(self, value: &T, buf: &mut String) {
                maud::display(value).render_to(buf);
            }
        }
    }

    // `key` arrives with its leading space (`" href"`), so an omitted
    // attribute leaves nothing behind.
    pub mod attr {
        use super::*;
        use crate::{Js, PreEscaped};

        #[derive(Clone, Copy, PartialEq)]
        pub enum Ctx {
            Text,
            /// `bool` is written as the word `true` or `false`.
            Words,
            /// `javascript:` and `vbscript:` URLs are blocked unless they
            /// come from `Js`.
            Url,
            /// The value is an HTML document, escaped once more so the
            /// attribute holds it as text.
            Srcdoc,
            Json,
        }

        const BLOCKED_URL: &str = "about:invalid#blocked";

        pub struct JsKind;
        pub struct MarkupKind;
        pub struct BoolKind;
        pub struct OptionKind;
        pub struct ValueKind;
        pub struct DisplayKind;

        pub trait ViaJs {
            fn htmloxide_attr_kind(&self) -> JsKind {
                JsKind
            }
        }
        pub trait ViaMarkup {
            fn htmloxide_attr_kind(&self) -> MarkupKind {
                MarkupKind
            }
        }
        pub trait ViaBool {
            fn htmloxide_attr_kind(&self) -> BoolKind {
                BoolKind
            }
        }
        pub trait ViaOption {
            fn htmloxide_attr_kind(&self) -> OptionKind {
                OptionKind
            }
        }
        pub trait ViaValue {
            fn htmloxide_attr_kind(&self) -> ValueKind {
                ValueKind
            }
        }
        pub trait ViaDisplay {
            fn htmloxide_attr_kind(&self) -> DisplayKind {
                DisplayKind
            }
        }

        impl<T: AsJs + ?Sized> ViaJs for &&&&&&Wrap<&T> {}
        impl<T: AsMarkup + ?Sized> ViaMarkup for &&&&&Wrap<&T> {}
        impl<T: AsBool + ?Sized> ViaBool for &&&&Wrap<&T> {}
        impl<T: Render> ViaOption for &&&Wrap<&Option<T>> {}
        impl<T: Render + ?Sized> ViaValue for &&Wrap<&T> {}
        impl<T: Display + ?Sized> ViaDisplay for &Wrap<&T> {}

        #[diagnostic::on_unimplemented(
            message = "`{Self}` can't be the value of an attribute that runs JavaScript",
            label = "`on*`, `hx-on*` and `hx-vars` attributes take `Js`",
            note = "write the code with `htmloxide::js!(\"..\")`; values in `{{..}}` are written as JSON"
        )]
        pub trait AsJs {
            fn as_js(&self) -> Option<&Js>;
        }
        impl AsJs for Js {
            fn as_js(&self) -> Option<&Js> {
                Some(self)
            }
        }
        impl AsJs for Option<Js> {
            fn as_js(&self) -> Option<&Js> {
                self.as_ref()
            }
        }
        impl<T: AsJs + ?Sized> AsJs for &T {
            fn as_js(&self) -> Option<&Js> {
                (**self).as_js()
            }
        }

        pub trait AsMarkup {
            fn as_markup(&self) -> Option<&str>;
        }
        impl<T: AsRef<str>> AsMarkup for PreEscaped<T> {
            fn as_markup(&self) -> Option<&str> {
                Some(self.0.as_ref())
            }
        }
        impl<T: AsRef<str>> AsMarkup for Option<PreEscaped<T>> {
            fn as_markup(&self) -> Option<&str> {
                self.as_ref().map(|m| m.0.as_ref())
            }
        }
        impl<T: AsMarkup + ?Sized> AsMarkup for &T {
            fn as_markup(&self) -> Option<&str> {
                (**self).as_markup()
            }
        }

        pub trait AsBool {
            fn as_bool(&self) -> Option<bool>;
        }
        impl AsBool for bool {
            fn as_bool(&self) -> Option<bool> {
                Some(*self)
            }
        }
        impl AsBool for Option<bool> {
            fn as_bool(&self) -> Option<bool> {
                *self
            }
        }
        impl<T: AsBool + ?Sized> AsBool for &T {
            fn as_bool(&self) -> Option<bool> {
                (**self).as_bool()
            }
        }

        /// `f` writes the value as it would be written as a child. Escaped
        /// text never contains `"`, but a `Render` impl may write one, which
        /// would end the attribute early.
        fn write_value(key: &str, buf: &mut String, ctx: Ctx, f: impl FnOnce(&mut String)) {
            buf.push_str(key);
            buf.push_str("=\"");
            let start = buf.len();
            f(buf);
            if ctx == Ctx::Srcdoc {
                let html = buf.split_off(start);
                escape_into(buf, &html);
            } else if buf[start..].contains('"') {
                let value = buf.split_off(start);
                buf.push_str(&value.replace('"', "&quot;"));
            }
            if ctx == Ctx::Url && is_script_url(&buf[start..]) {
                buf.truncate(start);
                buf.push_str(BLOCKED_URL);
            }
            buf.push('"');
        }

        /// For HTML or JSON: escaped as a whole, so that the browser decodes
        /// the attribute back to exactly `value`.
        pub(super) fn write_unescaped(key: &str, buf: &mut String, ctx: Ctx, value: &str) {
            buf.push_str(key);
            buf.push_str("=\"");
            if ctx == Ctx::Url && is_script_url(value) {
                buf.push_str(BLOCKED_URL);
            } else {
                escape_into(buf, value);
            }
            buf.push('"');
        }

        pub(super) fn write_js(key: &str, buf: &mut String, prefix: &str, js: &Js) {
            buf.push_str(key);
            buf.push_str("=\"");
            escape_into(buf, prefix);
            escape_into(buf, js.as_str());
            buf.push('"');
        }

        fn escape_into(buf: &mut String, s: &str) {
            use std::fmt::Write;
            let _ = maud::Escaper::new(buf).write_str(s);
        }

        /// Reads the scheme as a browser does: leading spaces and control
        /// characters are skipped, tabs and line breaks are ignored, and
        /// case does not matter.
        fn is_script_url(url: &str) -> bool {
            let chars = url
                .chars()
                .skip_while(|c| *c <= ' ')
                .filter(|c| !matches!(c, '\t' | '\n' | '\r'));
            ["javascript:", "vbscript:"].iter().any(|scheme| {
                let mut chars = chars.clone();
                scheme
                    .chars()
                    .all(|s| chars.next().is_some_and(|c| c.eq_ignore_ascii_case(&s)))
            })
        }

        impl JsKind {
            #[inline]
            pub fn render<T: AsJs + ?Sized>(
                self,
                key: &str,
                value: &T,
                buf: &mut String,
                ctx: Ctx,
            ) {
                if let Some(js) = value.as_js() {
                    let prefix = match ctx {
                        Ctx::Url => "javascript:",
                        Ctx::Json => "js:",
                        Ctx::Text | Ctx::Words | Ctx::Srcdoc => "",
                    };
                    write_js(key, buf, prefix, js);
                }
            }
        }
        impl MarkupKind {
            #[inline]
            pub fn render<T: AsMarkup + ?Sized>(
                self,
                key: &str,
                value: &T,
                buf: &mut String,
                ctx: Ctx,
            ) {
                if let Some(html) = value.as_markup() {
                    write_unescaped(key, buf, ctx, html);
                }
            }
        }
        impl BoolKind {
            #[inline]
            pub fn render<T: AsBool + ?Sized>(
                self,
                key: &str,
                value: &T,
                buf: &mut String,
                ctx: Ctx,
            ) {
                match value.as_bool() {
                    Some(value) if ctx == Ctx::Words => write_value(key, buf, ctx, |b| {
                        b.push_str(if value { "true" } else { "false" })
                    }),
                    Some(true) => buf.push_str(key),
                    Some(false) | None => {}
                }
            }
        }
        impl OptionKind {
            #[inline]
            pub fn render<T: Render>(
                self,
                key: &str,
                value: &Option<T>,
                buf: &mut String,
                ctx: Ctx,
            ) {
                if let Some(v) = value {
                    write_value(key, buf, ctx, |b| v.render_to(b));
                }
            }
        }
        impl ValueKind {
            #[inline]
            pub fn render<T: Render + ?Sized>(
                self,
                key: &str,
                value: &T,
                buf: &mut String,
                ctx: Ctx,
            ) {
                write_value(key, buf, ctx, |b| value.render_to(b));
            }
        }
        impl DisplayKind {
            #[inline]
            pub fn render<T: Display + ?Sized>(
                self,
                key: &str,
                value: &T,
                buf: &mut String,
                ctx: Ctx,
            ) {
                write_value(key, buf, ctx, |b| maud::display(value).render_to(b));
            }
        }
    }

    pub mod attr_json {
        use super::*;

        pub struct JsonKind;

        pub trait ViaJson {
            fn htmloxide_attr_kind(&self) -> JsonKind {
                JsonKind
            }
        }
        impl<T: serde::Serialize + ?Sized> ViaJson for Wrap<&T> {}

        impl JsonKind {
            #[inline]
            pub fn render<T: serde::Serialize + ?Sized>(
                self,
                key: &str,
                value: &T,
                buf: &mut String,
                ctx: attr::Ctx,
            ) {
                let mut json = String::new();
                write_json(value, &mut json);
                attr::write_unescaped(key, buf, ctx, &json);
            }
        }
    }

    /// `on*`, `hx-on*` and `hx-vars` accept only `Js`, so a string can never
    /// become a handler.
    #[inline]
    pub fn attr_js<T: attr::AsJs + ?Sized>(key: &str, value: &T, buf: &mut String) {
        if let Some(js) = value.as_js() {
            attr::write_js(key, buf, "", js);
        }
    }

    #[diagnostic::on_unimplemented(
        message = "`{Self}` can't be the URL of a `<script src>` or `<base href>`",
        label = "a URL computed at runtime would decide which code runs",
        note = "use a string literal or a `&'static str` constant"
    )]
    pub trait StaticUrl {
        fn as_static(&self) -> Option<&'static str>;
    }
    impl StaticUrl for &'static str {
        fn as_static(&self) -> Option<&'static str> {
            Some(self)
        }
    }
    impl StaticUrl for Option<&'static str> {
        fn as_static(&self) -> Option<&'static str> {
            *self
        }
    }

    #[inline]
    pub fn attr_static<T: StaticUrl + ?Sized>(key: &str, value: &T, buf: &mut String) {
        if let Some(url) = value.as_static() {
            attr::write_unescaped(key, buf, attr::Ctx::Url, url);
        }
    }

    // No tiers are needed below: `Js` does not implement `serde::Serialize`,
    // so the two impls of `ScriptValue`, and of `JsPart`, never overlap.

    #[inline]
    pub fn script_value<T: ScriptValue + ?Sized>(value: &T, buf: &mut String) {
        value.write_script(buf);
    }

    #[inline]
    pub fn style_value<T: StyleValue + ?Sized>(value: &T, buf: &mut String) {
        value.write_style(buf);
    }

    #[inline]
    pub fn js_part<T: JsPart + ?Sized>(value: &T, buf: &mut String) {
        value.write_js_part(buf);
    }

    #[inline]
    pub fn js_from_template(code: String) -> crate::Js {
        crate::Js(std::borrow::Cow::Owned(code))
    }

    #[diagnostic::on_unimplemented(
        message = "`{Self}` can't be written inside `<script>`",
        label = "values inside `<script>` are JSON data, or `Js` code",
        note = "data: any `serde::Serialize` type",
        note = "code: `htmloxide::js!(\"..\")`, or `Js::from_static(..)` for code compiled into the program"
    )]
    pub trait ScriptValue {
        fn write_script(&self, buf: &mut String);
    }
    impl ScriptValue for crate::Js {
        fn write_script(&self, buf: &mut String) {
            write_guarded(buf, self.as_str(), "script", true, |s| {
                if s == b's' { "\\u0073" } else { "\\u0053" }
            });
        }
    }
    impl<T: serde::Serialize + ?Sized> ScriptValue for T {
        fn write_script(&self, buf: &mut String) {
            write_json(self, buf);
        }
    }

    #[diagnostic::on_unimplemented(
        message = "`{Self}` can't be a value in `js!`",
        label = "values in `js!` are written as JSON data",
        note = "data: any `serde::Serialize` type"
    )]
    pub trait JsPart {
        fn write_js_part(&self, buf: &mut String);
    }
    impl JsPart for crate::Js {
        fn write_js_part(&self, buf: &mut String) {
            buf.push_str(self.as_str());
        }
    }
    impl<T: serde::Serialize + ?Sized> JsPart for T {
        fn write_js_part(&self, buf: &mut String) {
            write_json(self, buf);
        }
    }

    #[diagnostic::on_unimplemented(
        message = "`{Self}` can't be written inside `<style>`",
        label = "values inside `<style>` are CSS text",
        note = "pass the CSS as a string"
    )]
    pub trait StyleValue {
        fn write_style(&self, buf: &mut String);
    }
    impl<T: AsRef<str> + ?Sized> StyleValue for T {
        fn write_style(&self, buf: &mut String) {
            write_guarded(buf, self.as_ref(), "style", false, |s| {
                if s == b's' { "\\73 " } else { "\\53 " }
            });
        }
    }

    /// A browser ends the element at the first `</tag`, in any case, even
    /// inside a string; inside `<script>`, a `<script` after `<!--` makes it
    /// skip the next `</script>`. The first letter of each such tag name is
    /// replaced with `escape(letter)`, an escape that JavaScript or CSS reads
    /// as the same letter.
    fn write_guarded(
        buf: &mut String,
        code: &str,
        tag: &str,
        opening: bool,
        escape: fn(u8) -> &'static str,
    ) {
        let bytes = code.as_bytes();
        let mut written = 0;
        for (lt, _) in code.match_indices('<') {
            let mut name = lt + 1;
            if bytes.get(name) == Some(&b'/') {
                name += 1;
            } else if !opening {
                continue;
            }
            let matches = bytes
                .get(name..name + tag.len())
                .is_some_and(|n| n.eq_ignore_ascii_case(tag.as_bytes()));
            if matches {
                // `name` follows an ASCII byte, so it is a char boundary.
                buf.push_str(&code[written..name]);
                buf.push_str(escape(bytes[name]));
                written = name + 1;
            }
        }
        buf.push_str(&code[written..]);
    }

    /// `<`, `>`, `&`, U+2028 and U+2029 are written as `\u` escapes: `<` and
    /// `>` so the JSON cannot end a `<script>`, `&` so it reads the same in an
    /// attribute, and U+2028 / U+2029 because older JavaScript ends a string
    /// at them. They occur only inside JSON strings, where the escapes mean
    /// the same characters.
    fn write_json<T: serde::Serialize + ?Sized>(value: &T, buf: &mut String) {
        use std::fmt::Write;
        match serde_json::to_string(value) {
            Ok(json) => {
                for c in json.chars() {
                    match c {
                        '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                            let _ = write!(buf, "\\u{:04x}", c as u32);
                        }
                        c => buf.push(c),
                    }
                }
            }
            // Only for values JSON cannot hold, such as a map with
            // non-string keys.
            Err(_) => buf.push_str("null"),
        }
    }
}
