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
//! * [`#[component]`](component) turns a function into a `<Tag prop=..>` with
//!   compile-time-checked props.
//! * [`html_async!`] does the same for async components and evaluates to a future.
//!
//! Anything implementing [`Render`] (or `Display`) can be used inside `{..}`.

// htmloxide is split in two crates because a `proc-macro = true` crate can
// only export macros. The macros are in `htmloxide-macros`; everything the
// code they generate needs at runtime is in this crate, under `__private`.
// Users depend on this crate only.

// --------------------------------------------------
// Public API: re-exports
// --------------------------------------------------

// Generated code refers to this crate as `::htmloxide::..`. That path works
// in crates that depend on htmloxide; this line makes it work inside
// htmloxide itself too, for unit tests that use the macros.
extern crate self as htmloxide;

use std::cell::Cell;

pub use htmloxide_macros::{component, html, html_async};
// The rendering types come from maud: anything that renders in maud renders
// here too, and `Markup` from either crate can be mixed freely.
pub use maud::{Escaper, Markup, PreEscaped, Render, display};

// --------------------------------------------------
// Public API: rendering iterators
// --------------------------------------------------

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
// `Render::render_to` only gets `&self`, but running the iterator needs to
// own it. `Cell::take` moves the iterator out through the shared reference,
// leaving `None` behind.
pub struct Each<I>(Cell<Option<I>>);

/// Await a collection of futures *concurrently* and concatenate their
/// markup in order. Use inside `html_async!`, typically with async
/// components:
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
    // `join_all` runs all the futures at the same time and returns their
    // outputs in the original order, whatever order they finish in.
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

// --------------------------------------------------
// Internals: support for the generated code
// --------------------------------------------------

/// Implementation details used by the generated code. Not public API.
///
/// Dispatch is done with "autoref specialization": the macro calls a method
/// on `&&&&Wrap(value)` and method resolution picks the impl that needs the
/// fewest auto-derefs, i.e. the first tier whose bounds hold.
//
// The problem it solves: the code for `{expr}` has to do different things
// depending on the type of `expr` (render it, loop over it, reject it, format
// it). A proc macro only sees tokens, so it can't choose, and stable Rust has
// no specialization to choose between overlapping impls either.
//
// The trick: on a method call, the compiler tries the receiver type as
// written first, then removes one `&` at a time, and stops at the first type
// that has a matching method. So we give each behaviour (a "tier") its own
// trait, with the same method name, implemented for a different number of
// `&`s:
//
//     impl<T: Render>  ViaRender  for &&&Wrap<&T>  // tried first
//     impl<T: Display> ViaDisplay for    Wrap<&T>  // tried last
//
// Calling `(&&&&Wrap(value)).htmloxide_node_kind()` lands on the first tier
// whose bounds `T` meets. Each tier's method returns a different zero-sized
// "kind" value (`RenderKind`, `DisplayKind`, ..), and that kind's `render`
// method does the actual work.
#[doc(hidden)]
pub mod __private {
    use std::fmt::Display;

    use maud::Render;

    /// The receiver for the dispatch methods.
    ///
    /// Each tier is implemented for `Wrap<&T>` behind a set number of `&`s,
    /// so it matches at exactly one step of the method lookup. Implemented on
    /// a bare `T` instead, the last tier (`impl<T: Display> .. for T`) would
    /// also match `&&&T` and collide with the first.
    pub struct Wrap<T>(pub T);

    // --------------------------------------------------
    // Components: props builder states
    // --------------------------------------------------

    /// Builder state: prop not provided yet.
    pub struct Unset;
    /// Builder state: prop provided.
    pub struct Set<T>(pub T);

    /// Implemented by every `#[component]`'s props struct. Links the struct to
    /// its builder's starting state, where every prop is `Unset`.
    pub trait Props {
        type Builder;
        fn builder() -> Self::Builder;
    }

    /// `props_builder(&Card)` -> `CardProps::builder()`, with the props type
    /// inferred from the component's signature, so `<ui::Card>` works without
    /// importing `CardProps`.
    //
    // The argument's value is never used, only its type: `Fn(P) -> R` lets
    // the compiler work out `P`, the props struct.
    #[inline]
    pub fn props_builder<P: Props, R>(_component: &impl Fn(P) -> R) -> P::Builder {
        P::builder()
    }

    /// Props with a default: either the provided value or the default.
    ///
    /// The default comes as a closure, so `#[prop(default = expensive())]`
    /// only runs `expensive()` when the prop is left out.
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

    // --------------------------------------------------
    // Macros: render! and render_async!
    // --------------------------------------------------
    //
    // `#[macro_export]` is how a `macro_rules!` macro leaves its crate, and it
    // always places the macro at the crate root. The `__htmloxide_` prefix
    // and `#[doc(hidden)]` keep these out of the way there, and each `pub use`
    // makes the macro reachable as `::htmloxide::__private::render!`, the path
    // the generated code uses.
    //
    // In these macros, `match` evaluates the expression once and keeps any
    // temporaries it creates alive until rendering is done. `use ..::*`
    // brings the tier traits into scope, since a trait's methods can only be
    // called while the trait is in scope.

    /// `{expr}` as a child node. Tiers: `Render` > iterable of `Render` >
    /// future (compile error pointing at `html_async!`) > `Display`.
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

    /// Async-mode child (`html_async!`): takes the value, awaits it if it's
    /// a future, then renders the result like `render!`.
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

    /// Two tiers for `render_async!`: anything `IntoFuture` > everything else.
    ///
    /// Both kinds return a future: the real one, or `ready(value)`, which
    /// completes immediately. That way the macro can `.await` the result
    /// without knowing which kind it got.
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

    // --------------------------------------------------
    // Macros: attr!
    // --------------------------------------------------
    //
    // Built the same way as `render!`, with the attribute's key passed along
    // to the kind's `render`.

    /// `key={expr}`. Tiers: `bool` (toggle) > `Option<impl Render>` (omit on
    /// `None`) > `Render` > `Display`.
    #[macro_export]
    #[doc(hidden)]
    macro_rules! __htmloxide_attr {
        ($key:expr, $value:expr, $buf:expr) => {
            match $value {
                value => {
                    #[allow(unused_imports)]
                    use $crate::__private::attr::*;
                    (&&&&$crate::__private::Wrap(value))
                        .htmloxide_attr_kind()
                        .render($key, value, $buf)
                }
            }
        };
    }
    pub use crate::__htmloxide_attr as attr;

    // --------------------------------------------------
    // Dispatch: {expr} children
    // --------------------------------------------------

    /// The tiers behind `render!`, from first tried to last.
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

        /// Never implemented: exists only to turn "a future ended up in
        /// sync `html!`" into a readable error.
        #[diagnostic::on_unimplemented(
            message = "async component or future used inside `html!`",
            label = "this is a future",
            note = "use `html_async! {{ .. }}.await` to render async components"
        )]
        pub trait UseHtmlAsync {}

        // One `&` fewer per tier: `Render` is tried first, `Display` last.
        // Futures come before `Display`, so a future gets the error about
        // `html_async!` instead of a confusing "doesn't implement Display".
        impl<T: Render + ?Sized> ViaRender for &&&Wrap<&T> {}
        impl<T: std::future::Future + ?Sized> ViaFuture for &Wrap<&T> {}
        // `&Vec<T>`, `&[T]`, `&[T; N]` and `&Option<T>` all iterate over
        // their items.
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
        // No type implements `UseHtmlAsync`, so a call to this method never
        // compiles and the body never runs.
        impl FutureKind {
            pub fn render<T: ?Sized + UseHtmlAsync>(self, _: &T, _: &mut String) {
                unreachable!()
            }
        }
        impl DisplayKind {
            // `maud::display` escapes the formatted text.
            #[inline]
            pub fn render<T: Display + ?Sized>(self, value: &T, buf: &mut String) {
                maud::display(value).render_to(buf);
            }
        }
    }

    // --------------------------------------------------
    // Dispatch: key={expr} attributes
    // --------------------------------------------------

    /// The tiers behind `attr!`, from first tried to last.
    ///
    /// `key` arrives with its leading space (`" href"`), so a dropped
    /// attribute leaves nothing behind in the tag.
    pub mod attr {
        use super::*;

        pub struct BoolKind;
        pub struct OptionKind;
        pub struct ValueKind;
        pub struct DisplayKind;

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

        impl ViaBool for &&&Wrap<&bool> {}
        impl<T: Render> ViaOption for &&Wrap<&Option<T>> {}
        impl<T: Render + ?Sized> ViaValue for &Wrap<&T> {}
        impl<T: Display + ?Sized> ViaDisplay for Wrap<&T> {}

        /// Writes ` key="value"`, with `f` writing the value.
        fn write_value(key: &str, buf: &mut String, f: impl FnOnce(&mut String)) {
            buf.push_str(key);
            buf.push_str("=\"");
            f(buf);
            buf.push('"');
        }

        // `checked={true}` writes ` checked`; `checked={false}` writes nothing.
        impl BoolKind {
            #[inline]
            pub fn render(self, key: &str, value: &bool, buf: &mut String) {
                if *value {
                    buf.push_str(key);
                }
            }
        }
        // `title={Some("x")}` writes ` title="x"`; `None` writes nothing.
        impl OptionKind {
            #[inline]
            pub fn render<T: Render>(self, key: &str, value: &Option<T>, buf: &mut String) {
                if let Some(v) = value {
                    write_value(key, buf, |b| v.render_to(b));
                }
            }
        }
        impl ValueKind {
            #[inline]
            pub fn render<T: Render + ?Sized>(self, key: &str, value: &T, buf: &mut String) {
                write_value(key, buf, |b| value.render_to(b));
            }
        }
        impl DisplayKind {
            #[inline]
            pub fn render<T: Display + ?Sized>(self, key: &str, value: &T, buf: &mut String) {
                write_value(key, buf, |b| maud::display(value).render_to(b));
            }
        }
    }
}
