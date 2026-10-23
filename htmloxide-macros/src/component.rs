//! The `#[component]` attribute macro.
//!
//! A component is a plain Rust function that `html!` can use as a tag, with
//! the function's arguments as the tag's attributes ("props"):
//!
//! ```ignore
//! #[component]
//! fn Card(title: &str, #[prop(default)] featured: bool, children: Markup) -> Markup { .. }
//!
//! html! { <Card title="Hello" featured><p>"Body"</p></Card> }
//! ```
//!
//! In the tag, props are matched by *name*, can come in any order, and some
//! can be left out. A Rust function call only has positional arguments, so we
//! rewrite the function to take one struct, and generate a builder that fills
//! that struct one named prop at a time. For `Card`, the output is roughly:
//!
//! ```ignore
//! // The props struct: one field per original argument.
//! struct CardProps<'props> { pub title: &'props str, pub featured: bool, pub children: Markup }
//!
//! // The builder: one field and one type parameter per prop.
//! struct CardPropsBuilder<__F0, __F1, __F2> { title: __F0, featured: __F1, children: __F2 }
//!
//! // The original function, now taking the struct. Its first statement
//! // unpacks the struct, so the original body runs unchanged.
//! fn Card<'props>(props: CardProps<'props>) -> Markup {
//!     let CardProps { title, featured, children } = props;
//!     ..
//! }
//! ```
//!
//! and `html!` turns the tag into
//! `Card(props_builder(&Card).title("Hello").featured(true).children(..).build())`.
//!
//! The builder is a *type-state* builder: each type parameter `__Fi` is either
//! `Unset` or `Set<T>`, so the builder's type records which props have been
//! given. A setter can only be called while its
//! prop is `Unset`, and `build()` can only be called once every required prop
//! is `Set`. A missing or duplicated prop is therefore a type error, caught at
//! compile time, and `#[diagnostic::on_unimplemented]` turns that error into a
//! readable message such as "missing required prop `title` on `<Card>`".

use std::collections::HashSet;

use proc_macro2::{Span, TokenStream, TokenTree};
use quote::{ToTokens, format_ident, quote};
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::visit_mut::VisitMut;
use syn::{
    FnArg, GenericArgument, GenericParam, Generics, ItemFn, Lifetime, LifetimeParam, Pat,
    PathArguments, Type, WherePredicate,
};

// --------------------------------------------------
// Data: what we record about each prop
// --------------------------------------------------

/// The lifetime that replaces elided lifetimes in prop types.
///
/// `fn Card(title: &str)` is valid for a function, but a struct field can't
/// leave its lifetime out, so `CardProps` gets `title: &'props str`.
const PROPS_LIFETIME: &str = "'props";

/// One argument of the component function, read from its signature and its
/// `#[prop(..)]` attributes.
struct Prop {
    /// The argument name, which is also the prop name used in `html!`.
    ident: syn::Ident,
    /// Kept so that `mut count: u32` is still mutable inside the body.
    mutability: Option<syn::Token![mut]>,
    /// The argument type, with elided lifetimes already rewritten to `'props`.
    ty: Type,
    /// `#[prop(into)]`: the setter accepts anything that converts `Into` `ty`.
    into: bool,
    default: PropDefault,
}

/// What `build()` uses when the prop was left out of the tag.
enum PropDefault {
    /// Nothing: leaving the prop out is a compile error.
    Required,
    /// `#[prop(default)]` or `#[prop(default = expr)]`: the fallback
    /// expression (`Default::default()` for the bare form).
    Value(TokenStream),
    /// `#[prop(optional)]` on `Option<T>`: holds `T`. The setter takes a `T`
    /// and wraps it in `Some`; leaving the prop out gives `None`.
    Optional(Box<Type>),
}

// --------------------------------------------------
// Entry point: expand
// --------------------------------------------------

/// Expands `#[component] fn Name(..)` into the function, its props struct and
/// its builder.
pub fn expand(item: ItemFn) -> syn::Result<TokenStream> {
    let ItemFn {
        attrs,
        vis,
        sig,
        block,
        ..
    } = item;

    // Every generated item is named after the component (`Card` gives
    // `CardProps` and `CardPropsBuilder`), so two components in the same
    // module never clash.
    let name = &sig.ident;
    let props_name = format_ident!("{}Props", name);
    let builder_name = format_ident!("{}PropsBuilder", name);
    let output = &sig.output;
    let asyncness = &sig.asyncness;

    // --------------------------------------------------
    // Step 1: read each argument into a `Prop`
    // --------------------------------------------------
    //
    // Each argument must be a plain `name: Type`, because the name becomes the
    // prop name in `html!`. Elided lifetimes in the type are rewritten to
    // `'props`, and the `#[prop(..)]` attributes mark the prop as required or
    // not.

    let mut generics = sig.generics.clone();
    let mut elided = ElidedLifetimes::default();
    let mut props = Vec::new();

    for arg in &sig.inputs {
        let arg = match arg {
            FnArg::Typed(arg) => arg,
            FnArg::Receiver(r) => {
                return Err(syn::Error::new(r.span(), "components can't take `self`"));
            }
        };
        let Pat::Ident(pat) = &*arg.pat else {
            return Err(syn::Error::new(
                arg.pat.span(),
                "component arguments must be plain identifiers (they become prop names)",
            ));
        };
        // `ref x` and `x @ pattern` are rejected: step 6 only reproduces `x`
        // and `mut x` when it unpacks the props struct.
        if pat.by_ref.is_some() || pat.subpat.is_some() {
            return Err(syn::Error::new(pat.span(), "unsupported argument pattern"));
        }

        let mut ty = (*arg.ty).clone();
        elided.visit_type_mut(&mut ty);
        // A struct field can't have an `impl Trait` type, so there's nowhere
        // to store such a prop.
        if let Some(span) = elided.impl_trait.take() {
            return Err(syn::Error::new(
                span,
                "`impl Trait` props aren't supported; use a generic parameter instead",
            ));
        }

        let (into, default) = parse_prop_attrs(&arg.attrs, &ty)?;
        props.push(Prop {
            ident: pat.ident.clone(),
            mutability: pat.mutability,
            ty,
            into,
            default,
        });
    }

    // If any prop type now mentions `'props`, every generated item needs it as
    // a generic parameter, unless the function already declares it.
    if elided.used {
        let exists = generics
            .lifetimes()
            .any(|l| l.lifetime.to_string() == PROPS_LIFETIME);
        if !exists {
            let param = LifetimeParam::new(Lifetime::new(PROPS_LIFETIME, Span::call_site()));
            generics.params.insert(0, GenericParam::Lifetime(param));
        }
    }

    // The three ways the generics get written in generated code:
    // `impl_g` after `impl` / `fn` (`<'props, T: Bound>`), `ty_g` after a type
    // name (`<'props, T>`), and `where_c` for the `where` clause.
    let (impl_g, ty_g, where_c) = generics.split_for_impl();
    let all_names = generic_names(&generics);

    // --------------------------------------------------
    // Step 2: the props struct
    // --------------------------------------------------
    //
    // `struct CardProps<'props> { pub title: &'props str, .. }`, with the same
    // visibility and generics as the function. The fields are `pub`, so code
    // outside `html!` can also fill a `CardProps` and call `Card(props)`.

    let field_defs = props.iter().map(|p| {
        let (ident, ty) = (&p.ident, &p.ty);
        quote!(pub #ident: #ty)
    });
    // `allow`: the struct is named after the function, which may not be
    // CamelCase.
    let props_struct = quote! {
        #[allow(non_camel_case_types)]
        #vis struct #props_name #impl_g #where_c {
            #( #field_defs, )*
        }
    };

    // --------------------------------------------------
    // Step 3: the builder and its empty starting state
    // --------------------------------------------------
    //
    // The builder has one type parameter per prop (`__F0`, `__F1`, ..), and
    // each field holds a value of that type: `Unset` or `Set<T>`.
    //
    // The `Props` impl links `CardProps` to `CardPropsBuilder<Unset, ..>`, the
    // state where nothing is set yet. `props_builder(&Card)` follows that link
    // to start a builder from the function alone, without `html!` having to
    // name `CardProps`.

    let state: Vec<syn::Ident> = (0..props.len())
        .map(|i| format_ident!("__F{}", i))
        .collect();
    let idents: Vec<&syn::Ident> = props.iter().map(|p| &p.ident).collect();
    let unset = quote!(::htmloxide::__private::Unset);

    // `doc(hidden)`: users never write the builder's name themselves.
    let builder_struct = quote! {
        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        #vis struct #builder_name<#(#state),*> {
            #( #idents: #state, )*
        }
    };

    let all_unset = props.iter().map(|_| &unset);
    let props_impl = quote! {
        impl #impl_g ::htmloxide::__private::Props for #props_name #ty_g #where_c {
            type Builder = #builder_name<#(#all_unset),*>;
            fn builder() -> Self::Builder {
                #builder_name { #( #idents: ::htmloxide::__private::Unset, )* }
            }
        }
    };

    // --------------------------------------------------
    // Step 4: one setter method per prop
    // --------------------------------------------------
    //
    // A setter consumes the builder and returns a new one with this prop's
    // slot changed from `Unset` to `Set(value)`. For `title`:
    //
    //     impl<__F0, __F1, __F2> CardPropsBuilder<__F0, __F1, __F2> {
    //         pub fn title<'props>(self, __value: &'props str)
    //             -> CardPropsBuilder<Set<&'props str>, __F1, __F2>
    //         where __F0: __htmloxide_Card_title_once
    //         { .. }
    //     }
    //
    // The `where` bound rejects setting a prop twice. The
    // `__htmloxide_Card_title_once` trait is implemented only for `Unset`, so
    // after the first call `__F0` is `Set<..>` and a second call fails to
    // compile. `#[diagnostic::on_unimplemented]` swaps the compiler's generic
    // "trait bound not satisfied" message for "prop `title` is set more than
    // once on `<Card>`".

    let setters = props.iter().enumerate().map(|(i, p)| {
        let ident = &p.ident;
        let ty = &p.ty;
        let this_state = &state[i];
        let once = format_ident!("__htmloxide_{}_{}_once", name, ident);
        let once_msg = format!("prop `{ident}` is set more than once on `<{name}>`");
        // The builder type after this call: unchanged, except this prop's
        // slot, which becomes `Set<T>`.
        let out_state = state.iter().enumerate().map(|(j, s)| {
            if j == i {
                quote!(::htmloxide::__private::Set<#ty>)
            } else {
                s.to_token_stream()
            }
        });
        // The new builder's fields: the new value for this prop, and every
        // other field moved over as it was.
        let moves = idents.iter().enumerate().map(|(j, id)| {
            if j == i {
                quote!(#id: ::htmloxide::__private::Set(__value))
            } else {
                quote!(#id: self.#id)
            }
        });

        // A generic parameter that doesn't appear in the argument type can't
        // be inferred at the call site (`.title("x")` would fail with "type
        // annotations needed"). So each setter declares only the generics its
        // own prop type mentions.
        let method_g = subset_generics(&generics, &all_names, &mentioned(ty.to_token_stream()));
        let (m_impl, _, m_where) = method_g.split_for_impl();

        // What the setter accepts (`arg_ty`), and the expression that turns
        // `__value` into the field type (`convert`).
        let (arg_ty, convert) = match (&p.default, p.into) {
            (PropDefault::Optional(inner), true) => (
                quote!(impl ::core::convert::Into<#inner>),
                quote!(::core::option::Option::Some(__value.into())),
            ),
            (PropDefault::Optional(inner), false) => (
                quote!(#inner),
                quote!(::core::option::Option::Some(__value)),
            ),
            (_, true) => (
                quote!(impl ::core::convert::Into<#ty>),
                quote!(__value.into()),
            ),
            (_, false) => (quote!(#ty), quote!(__value)),
        };

        // The "set once" bound, plus the function's own `where` predicates
        // that only involve this setter's generics.
        let mut where_preds: Vec<TokenStream> = vec![quote!(#this_state: #once)];
        if let Some(w) = m_where {
            where_preds.extend(w.predicates.iter().map(ToTokens::to_token_stream));
        }

        quote! {
            #[doc(hidden)]
            #[allow(non_camel_case_types)]
            #[diagnostic::on_unimplemented(message = #once_msg, label = "remove the duplicate")]
            #vis trait #once {}
            impl #once for ::htmloxide::__private::Unset {}

            #[allow(non_camel_case_types)]
            impl<#(#state),*> #builder_name<#(#state),*> {
                pub fn #ident #m_impl (self, __value: #arg_ty) -> #builder_name<#(#out_state),*>
                where #( #where_preds, )*
                {
                    let __value: #ty = #convert;
                    #builder_name { #( #moves, )* }
                }
            }
        }
    });

    // --------------------------------------------------
    // Step 5: build(), which checks the required props
    // --------------------------------------------------
    //
    // `build()` turns the builder into `CardProps`. It puts one bound on each
    // builder slot, depending on the prop:
    //
    // - Required: a marker trait implemented only for `Set<T>`. If the prop
    //   was never set, its slot is `Unset`, the bound fails, and the error
    //   reads "missing required prop `title` on `<Card>`". The trait's one
    //   method takes the value back out of `Set<T>`.
    // - Default or optional: `PropOrDefault<T>`, implemented for `Set<T>`
    //   (gives the value) and for `Unset` (runs the default expression, or
    //   gives `None` for an optional prop).

    let mut marker_traits = Vec::new();
    let mut build_bounds = Vec::new();
    let mut build_fields = Vec::new();
    for (p, s) in props.iter().zip(&state) {
        let (ident, ty) = (&p.ident, &p.ty);
        match &p.default {
            PropDefault::Required => {
                let tr = format_ident!("__htmloxide_{}_{}", name, ident);
                let msg = format!("missing required prop `{ident}` on `<{name}>`");
                let label = format!("add `{ident}={{..}}` to this tag");
                marker_traits.push(quote! {
                    #[doc(hidden)]
                    #[allow(non_camel_case_types)]
                    #[diagnostic::on_unimplemented(message = #msg, label = #label)]
                    #vis trait #tr<T> { fn __htmloxide_take(self) -> T; }
                    impl<T> #tr<T> for ::htmloxide::__private::Set<T> {
                        fn __htmloxide_take(self) -> T { self.0 }
                    }
                });
                build_bounds.push(quote!(#s: #tr<#ty>));
                build_fields.push(quote!(#ident: #tr::__htmloxide_take(self.#ident)));
            }
            PropDefault::Value(expr) => {
                build_bounds.push(quote!(#s: ::htmloxide::__private::PropOrDefault<#ty>));
                build_fields.push(quote!(#ident: ::htmloxide::__private::PropOrDefault::take_or(self.#ident, || #expr)));
            }
            PropDefault::Optional(_) => {
                build_bounds.push(quote!(#s: ::htmloxide::__private::PropOrDefault<#ty>));
                build_fields.push(quote!(#ident: ::htmloxide::__private::PropOrDefault::take_or(self.#ident, || ::core::option::Option::None)));
            }
        }
    }
    // `build()` declares all of the function's generics, and the compiler
    // infers them from the `Set<T>` types the setters left in the slots. The
    // function's own `where` predicates apply here too.
    let user_preds = where_c
        .map(|w| w.predicates.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let build = quote! {
        impl<#(#state),*> #builder_name<#(#state),*> {
            pub fn build #impl_g (self) -> #props_name #ty_g
            where #( #build_bounds, )* #( #user_preds, )*
            {
                #props_name { #( #build_fields, )* }
            }
        }
    };

    // --------------------------------------------------
    // Step 6: the component function itself
    // --------------------------------------------------
    //
    // Same attributes, visibility, name, `async` and return type as the
    // original, but a single `props` argument. The first statement unpacks
    // `props` back into the original argument names, so the user's body runs
    // unchanged.

    let destructure = props.iter().map(|p| {
        let (m, id) = (&p.mutability, &p.ident);
        quote!(#m #id)
    });
    let body_stmts = &block.stmts;
    // `allow`: components are named like types (`Card`), which the
    // `non_snake_case` lint flags on a function.
    let func = quote! {
        #( #attrs )*
        #[allow(non_snake_case)]
        #vis #asyncness fn #name #impl_g (props: #props_name #ty_g) #output #where_c {
            let #props_name { #( #destructure, )* } = props;
            #( #body_stmts )*
        }
    };

    // --------------------------------------------------
    // Step 7: emit everything in place of the original function
    // --------------------------------------------------

    Ok(quote! {
        #func
        #props_struct
        #builder_struct
        #props_impl
        #( #setters )*
        #( #marker_traits )*
        #build
    })
}

// --------------------------------------------------
// Helpers: reading #[prop(..)] attributes
// --------------------------------------------------

/// Reads the `#[prop(..)]` attributes of one argument.
///
/// Returns whether `into` was given and what happens when the prop is left
/// out. `ty` is the argument type, needed to check that `optional` is only used
/// on an `Option<T>`.
fn parse_prop_attrs(attrs: &[syn::Attribute], ty: &Type) -> syn::Result<(bool, PropDefault)> {
    let mut into = false;
    let mut default = PropDefault::Required;
    for attr in attrs {
        if !attr.path().is_ident("prop") {
            return Err(syn::Error::new(
                attr.span(),
                "only `#[prop(..)]` is allowed on component arguments",
            ));
        }
        // The closure runs once per comma-separated item inside `prop(..)`:
        // twice for `#[prop(optional, into)]`.
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("into") {
                into = true;
            } else if meta.path.is_ident("default") {
                default = if meta.input.peek(syn::Token![=]) {
                    let expr: syn::Expr = meta.value()?.parse()?;
                    PropDefault::Value(expr.to_token_stream())
                } else {
                    PropDefault::Value(quote!(::core::default::Default::default()))
                };
            } else if meta.path.is_ident("optional") {
                let inner = option_inner(ty).ok_or_else(|| {
                    meta.error("`#[prop(optional)]` requires the prop type to be `Option<T>`")
                })?;
                default = PropDefault::Optional(Box::new(inner.clone()));
            } else {
                return Err(meta.error("expected `into`, `default`, `default = ..` or `optional`"));
            }
            Ok(())
        })?;
    }
    Ok((into, default))
}

/// For `Option<T>`, returns `T`.
///
/// Only the last path segment's name is checked, so `std::option::Option<T>`
/// matches too, but a type alias of `Option` doesn't.
fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(p) = ty else { return None };
    let seg = p.path.segments.last()?;
    if seg.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    match args.args.first()? {
        GenericArgument::Type(t) if args.args.len() == 1 => Some(t),
        _ => None,
    }
}

// --------------------------------------------------
// Visitor: naming elided lifetimes
// --------------------------------------------------

/// Rewrites the elided lifetimes in a prop type to `'props`: `&str` becomes
/// `&'props str`, and `Cow<'_, str>` becomes `Cow<'props, str>`.
///
/// syn's `VisitMut` walks every node of a type. Each method below overrides
/// how one kind of node is handled; all other nodes get the default walk.
///
/// `used` records whether anything was rewritten, which means the generated
/// items need a `'props` parameter. `impl_trait` holds the span of the first
/// `impl Trait` found, for the error message.
#[derive(Default)]
struct ElidedLifetimes {
    used: bool,
    impl_trait: Option<Span>,
}

impl VisitMut for ElidedLifetimes {
    // `&T` -> `&'props T`, then keep walking into `T`, which may contain more
    // references.
    fn visit_type_reference_mut(&mut self, r: &mut syn::TypeReference) {
        if r.lifetime.is_none() {
            r.lifetime = Some(Lifetime::new(PROPS_LIFETIME, r.and_token.span()));
            self.used = true;
        }
        syn::visit_mut::visit_type_reference_mut(self, r);
    }
    // `'_` -> `'props`. Named lifetimes such as `'a` are left alone.
    fn visit_lifetime_mut(&mut self, l: &mut Lifetime) {
        if l.ident == "_" {
            *l = Lifetime::new(PROPS_LIFETIME, l.span());
            self.used = true;
        }
    }
    // Lifetimes elided inside `fn(&str)` and `Fn(&str)` belong to that
    // signature ("any lifetime, chosen per call"), not to the component.
    // Naming them `'props` would change their meaning, so these two methods
    // don't walk inside.
    fn visit_type_fn_ptr_mut(&mut self, _: &mut syn::TypeFnPtr) {}
    fn visit_parenthesized_generic_arguments_mut(
        &mut self,
        _: &mut syn::ParenthesizedGenericArguments,
    ) {
    }
    fn visit_type_impl_trait_mut(&mut self, t: &mut syn::TypeImplTrait) {
        self.impl_trait.get_or_insert(t.span());
    }
}

// --------------------------------------------------
// Helpers: picking the generics for each setter
// --------------------------------------------------
//
// Each setter declares only the generic parameters its prop type uses. These
// helpers work that out by comparing *names*: a type uses `T` if the
// identifier `T` appears anywhere in it.

/// Names of all generic params (`'a`, `T`, `N`). Lifetimes keep their
/// apostrophe, so a lifetime `'a` and a type `a` stay distinct.
fn generic_names(g: &Generics) -> HashSet<String> {
    g.params
        .iter()
        .map(|p| match p {
            GenericParam::Lifetime(l) => l.lifetime.to_string(),
            GenericParam::Type(t) => t.ident.to_string(),
            GenericParam::Const(c) => c.ident.to_string(),
        })
        .collect()
}

/// Every identifier and lifetime that appears in `tokens`.
///
/// The result also holds names that aren't generics (`Vec<T>` gives both `Vec`
/// and `T`). That's harmless, since it's only ever compared with generic param
/// names.
fn mentioned(tokens: TokenStream) -> HashSet<String> {
    // Collects names from a parsed type.
    struct V(HashSet<String>);
    impl<'ast> Visit<'ast> for V {
        fn visit_ident(&mut self, i: &'ast proc_macro2::Ident) {
            self.0.insert(i.to_string());
        }
        fn visit_lifetime(&mut self, l: &'ast Lifetime) {
            self.0.insert(l.to_string());
        }
    }
    // Collects names from raw tokens, for input that isn't a type (such as the
    // where-predicate `T: Display`). A lifetime arrives as two tokens, `'` and
    // then `a`, so `prev_apostrophe` joins them back into `'a`.
    fn walk(ts: TokenStream, out: &mut HashSet<String>) {
        let mut prev_apostrophe = false;
        for tt in ts {
            match tt {
                TokenTree::Ident(i) => {
                    out.insert(if prev_apostrophe {
                        format!("'{i}")
                    } else {
                        i.to_string()
                    });
                    prev_apostrophe = false;
                }
                TokenTree::Punct(p) => prev_apostrophe = p.as_char() == '\'',
                TokenTree::Group(g) => {
                    walk(g.stream(), out);
                    prev_apostrophe = false;
                }
                TokenTree::Literal(_) => prev_apostrophe = false,
            }
        }
    }
    match syn::parse2::<Type>(tokens.clone()) {
        Ok(ty) => {
            let mut v = V(HashSet::new());
            v.visit_type(&ty);
            v.0
        }
        Err(_) => {
            let mut out = HashSet::new();
            walk(tokens, &mut out);
            out
        }
    }
}

/// A copy of `g` that keeps only the generic params named in `used`, and only
/// the `where` predicates that mention no other generic param. `all` holds
/// every generic param name of `g`.
fn subset_generics(g: &Generics, all: &HashSet<String>, used: &HashSet<String>) -> Generics {
    let keep: HashSet<&String> = all.iter().filter(|n| used.contains(*n)).collect();
    // `T: Into<U>` survives only if both `T` and `U` are kept.
    let only_kept = |tokens: TokenStream| {
        let m = mentioned(tokens);
        m.iter()
            .filter(|n| all.contains(*n))
            .all(|n| keep.contains(n))
    };

    let mut out = g.clone();
    out.params = g
        .params
        .iter()
        .filter(|p| {
            let n = match p {
                GenericParam::Lifetime(l) => l.lifetime.to_string(),
                GenericParam::Type(t) => t.ident.to_string(),
                GenericParam::Const(c) => c.ident.to_string(),
            };
            keep.contains(&n)
        })
        .cloned()
        .collect();
    if let Some(w) = &mut out.where_clause {
        w.predicates = w
            .predicates
            .iter()
            .filter(|p: &&WherePredicate| only_kept(p.to_token_stream()))
            .cloned()
            .collect();
    }
    if out.params.is_empty() {
        out.lt_token = None;
        out.gt_token = None;
    }
    out
}
