//! `#[component]`: a function that `html!` can use as a tag.
//!
//! ```ignore
//! #[component]
//! fn Card(title: &str, #[prop(default)] featured: bool, children: Markup) -> Markup { .. }
//!
//! html! { <Card title="Hello" featured><p>"Body"</p></Card> }
//! ```
//!
//! In a tag, props are matched by name, in any order, and some may be left
//! out. A Rust call has positional arguments only, so the function is
//! rewritten to take one struct, and a builder is generated that fills the
//! struct one named prop at a time:
//!
//! ```ignore
//! struct CardProps<'props> { pub title: &'props str, pub featured: bool, pub children: Markup }
//!
//! struct CardPropsBuilder<title, featured, children> { title: title, featured: featured, children: children }
//!
//! fn Card<'props>(props: CardProps<'props>) -> Markup {
//!     let CardProps { title, featured, children } = props;
//!     ..
//! }
//! ```
//!
//! `html!` turns the tag into
//! `Card(props_builder(&Card).title("Hello").featured(true).children(..).build())`.
//!
//! The builder is a type-state builder. Each type parameter is `Unset` or
//! `Set<T>`, so the builder's type encodes which props have been given. A
//! setter can only be called while its prop is `Unset`, and `build()` only
//! once every required prop is `Set`. A missing or repeated prop is
//! therefore a type error, and `#[diagnostic::on_unimplemented]` replaces
//! the compiler's message with one such as "missing required prop `title` on
//! `<Card>`". The type parameters are named after the props, so an error for
//! a misspelled prop lists the real ones.

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

/// A function argument may elide its lifetime (`&str`); a struct field may
/// not, so `CardProps` has `title: &'props str`.
const PROPS_LIFETIME: &str = "'props";

struct Prop {
    ident: syn::Ident,
    mutability: Option<syn::Token![mut]>,
    ty: Type,
    option_inner: Option<Type>,
    into: bool,
    default: PropDefault,
}

enum PropDefault {
    Required,
    Value(TokenStream),
}

pub fn expand(item: ItemFn) -> syn::Result<TokenStream> {
    let ItemFn {
        attrs,
        vis,
        sig,
        block,
        ..
    } = item;

    let name = &sig.ident;
    let props_name = format_ident!("{}Props", name);
    let builder_name = format_ident!("{}PropsBuilder", name);
    let output = &sig.output;
    let asyncness = &sig.asyncness;

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
        // The props struct is unpacked back into `x` or `mut x` only.
        if pat.by_ref.is_some() || pat.subpat.is_some() {
            return Err(syn::Error::new(pat.span(), "unsupported argument pattern"));
        }

        let mut ty = (*arg.ty).clone();
        elided.visit_type_mut(&mut ty);
        if let Some(span) = elided.impl_trait.take() {
            return Err(syn::Error::new(
                span,
                "`impl Trait` props aren't supported; use a generic parameter instead",
            ));
        }

        let (into, mut default) = parse_prop_attrs(&arg.attrs)?;
        let option_inner = option_inner(&ty).cloned();
        if option_inner.is_some() && matches!(default, PropDefault::Required) {
            default = PropDefault::Value(quote!(::core::option::Option::None));
        }
        props.push(Prop {
            ident: pat.ident.clone(),
            mutability: pat.mutability,
            ty,
            option_inner,
            into,
            default,
        });
    }

    if elided.used {
        let exists = generics
            .lifetimes()
            .any(|l| l.lifetime.to_string() == PROPS_LIFETIME);
        if !exists {
            let param = LifetimeParam::new(Lifetime::new(PROPS_LIFETIME, Span::call_site()));
            generics.params.insert(0, GenericParam::Lifetime(param));
        }
    }

    // `impl_g` follows `impl` or `fn` (`<'props, T: Bound>`), `ty_g` follows
    // a type name (`<'props, T>`), and `where_c` is the `where` clause.
    let (impl_g, ty_g, where_c) = generics.split_for_impl();
    let all_names = generic_names(&generics);

    let field_defs = props.iter().map(|p| {
        let (ident, ty) = (&p.ident, &p.ty);
        quote!(pub #ident: #ty)
    });
    let props_struct = quote! {
        #[allow(non_camel_case_types)]
        #vis struct #props_name #impl_g #where_c {
            #( #field_defs, )*
        }
    };

    // Inside the builder's impls, a type parameter shadows any type of the
    // same name. A prop named like a type that the generated code mentions
    // (`char: char`), or a raw identifier (`r#for`), gets a neutral parameter
    // name (`__F0`) instead.
    let mut taken = mentioned(generics.to_token_stream());
    taken.extend(mentioned(generics.where_clause.to_token_stream()));
    for p in &props {
        taken.extend(mentioned(p.ty.to_token_stream()));
        if let PropDefault::Value(expr) = &p.default {
            taken.extend(mentioned(expr.clone()));
        }
    }
    let state: Vec<syn::Ident> = props
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let name = p.ident.to_string();
            if name.starts_with("r#") || taken.contains(&name) {
                format_ident!("__F{}", i)
            } else {
                p.ident.clone()
            }
        })
        .collect();
    let idents: Vec<&syn::Ident> = props.iter().map(|p| &p.ident).collect();
    let unset = quote!(::htmloxide::__private::Unset);

    let builder_struct = quote! {
        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        #vis struct #builder_name<#(#state),*> {
            #( #idents: #state, )*
        }
    };

    // Lets `props_builder(&Card)` reach the empty builder from the function
    // alone, so `html!` never names `CardProps`.
    let all_unset = props.iter().map(|_| &unset);
    let props_impl = quote! {
        impl #impl_g ::htmloxide::__private::Props for #props_name #ty_g #where_c {
            type Builder = #builder_name<#(#all_unset),*>;
            fn builder() -> Self::Builder {
                #builder_name { #( #idents: ::htmloxide::__private::Unset, )* }
            }
        }
    };

    // Each setter consumes the builder and returns it with one type parameter
    // changed from `Unset` to `Set<T>`:
    //
    //     impl<title, featured, children> CardPropsBuilder<title, featured, children> {
    //         pub fn title<'props>(self, __value: &'props str)
    //             -> CardPropsBuilder<Set<&'props str>, featured, children>
    //         where title: __htmloxide_Card_title_once
    //         { .. }
    //     }
    //
    // The `_once` trait is implemented for `Unset` only, so a second call,
    // made when `title` is already `Set<..>`, does not compile.
    let setters = props.iter().enumerate().map(|(i, p)| {
        let ident = &p.ident;
        let ty = &p.ty;
        let this_state = &state[i];
        let once = format_ident!("__htmloxide_{}_{}_once", name, ident);
        let once_msg = format!("prop `{ident}` is set more than once on `<{name}>`");
        let out_state = state.iter().enumerate().map(|(j, s)| {
            if j == i {
                quote!(::htmloxide::__private::Set<#ty>)
            } else {
                s.to_token_stream()
            }
        });
        let moves = idents.iter().enumerate().map(|(j, id)| {
            if j == i {
                quote!(#id: ::htmloxide::__private::Set(__value))
            } else {
                quote!(#id: self.#id)
            }
        });

        // A generic parameter absent from the argument type cannot be
        // inferred at the call, so each setter declares only those its prop
        // type mentions.
        let mut method_g = subset_generics(&generics, &all_names, &mentioned(ty.to_token_stream()));

        let (arg_ty, convert) = match (&p.option_inner, p.into) {
            // `__K` selects between the two impls of `IntoOptionProp`.
            (Some(inner), true) => {
                method_g.params.push(syn::parse_quote!(__K));
                (
                    quote!(impl ::htmloxide::__private::IntoOptionProp<#inner, __K>),
                    quote!(::htmloxide::__private::IntoOptionProp::into_option_prop(
                        __value
                    )),
                )
            }
            (Some(_), false) | (None, true) => (
                quote!(impl ::core::convert::Into<#ty>),
                quote!(__value.into()),
            ),
            (None, false) => (quote!(#ty), quote!(__value)),
        };
        let (m_impl, _, m_where) = method_g.split_for_impl();

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

    // `build()` bounds each type parameter. A required prop's parameter must
    // implement a marker trait that only `Set<T>` implements, so an unset
    // prop fails the bound with "missing required prop". A prop with a
    // default needs `PropOrDefault`, which `Unset` implements by evaluating
    // the default.
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
        }
    }
    let user_preds = where_c
        .map(|w| w.predicates.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let build = quote! {
        #[allow(non_camel_case_types)]
        impl<#(#state),*> #builder_name<#(#state),*> {
            pub fn build #impl_g (self) -> #props_name #ty_g
            where #( #build_bounds, )* #( #user_preds, )*
            {
                #props_name { #( #build_fields, )* }
            }
        }
    };

    let destructure = props.iter().map(|p| {
        let (m, id) = (&p.mutability, &p.ident);
        quote!(#m #id)
    });
    let body_stmts = &block.stmts;
    let func = quote! {
        #( #attrs )*
        #[allow(non_snake_case)]
        #vis #asyncness fn #name #impl_g (props: #props_name #ty_g) #output #where_c {
            let #props_name { #( #destructure, )* } = props;
            #( #body_stmts )*
        }
    };

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

fn parse_prop_attrs(attrs: &[syn::Attribute]) -> syn::Result<(bool, PropDefault)> {
    let mut into = false;
    let mut default = PropDefault::Required;
    for attr in attrs {
        if !attr.path().is_ident("prop") {
            return Err(syn::Error::new(
                attr.span(),
                "only `#[prop(..)]` is allowed on component arguments",
            ));
        }
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
            } else {
                return Err(meta.error("expected `into`, `default` or `default = ..`"));
            }
            Ok(())
        })?;
    }
    Ok((into, default))
}

/// `T` for `Option<T>`. Matched by name, so a type alias of `Option` is not
/// recognized.
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

/// Names elided lifetimes `'props`: `&str` becomes `&'props str`, and
/// `Cow<'_, str>` becomes `Cow<'props, str>`.
#[derive(Default)]
struct ElidedLifetimes {
    used: bool,
    impl_trait: Option<Span>,
}

impl VisitMut for ElidedLifetimes {
    fn visit_type_reference_mut(&mut self, r: &mut syn::TypeReference) {
        if r.lifetime.is_none() {
            r.lifetime = Some(Lifetime::new(PROPS_LIFETIME, r.and_token.span()));
            self.used = true;
        }
        syn::visit_mut::visit_type_reference_mut(self, r);
    }
    fn visit_lifetime_mut(&mut self, l: &mut Lifetime) {
        if l.ident == "_" {
            *l = Lifetime::new(PROPS_LIFETIME, l.span());
            self.used = true;
        }
    }
    // A lifetime elided in `fn(&str)` or `Fn(&str)` means any lifetime,
    // chosen at each call. Naming it `'props` would change the type, so these
    // are not walked.
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

/// Every identifier and lifetime in `tokens`, compared by name against the
/// generic parameters to find which ones a type uses.
fn mentioned(tokens: TokenStream) -> HashSet<String> {
    struct V(HashSet<String>);
    impl<'ast> Visit<'ast> for V {
        fn visit_ident(&mut self, i: &'ast proc_macro2::Ident) {
            self.0.insert(i.to_string());
        }
        fn visit_lifetime(&mut self, l: &'ast Lifetime) {
            self.0.insert(l.to_string());
        }
    }
    // For tokens that are not a type, such as `T: Display`. A lifetime is two
    // tokens, `'` and `a`, joined back into `'a`.
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

/// `g` reduced to the parameters in `used`, and to the `where` predicates
/// that mention no other parameter.
fn subset_generics(g: &Generics, all: &HashSet<String>, used: &HashSet<String>) -> Generics {
    let keep: HashSet<&String> = all.iter().filter(|n| used.contains(*n)).collect();
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
