//! `js!`: JavaScript code with Rust values in it.
//!
//! ```ignore
//! js!("select({id}, {name})")   // select(7, "Ada")
//! ```
//!
//! The template is a string literal, so the code always comes from the
//! source. Each `{..}` is written at runtime as JSON, or as code if the value
//! is itself `Js`, so a value cannot change the code around it: a string
//! becomes a quoted JSON string, whatever it contains.
//!
//! Placeholders follow `format!`: `{id}`, `{}`, `{0}`, named arguments, and
//! `{{` / `}}` for literal braces. There are no format specs.

use std::collections::HashSet;

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, Ident, LitStr, Token};

struct Input {
    template: LitStr,
    args: Vec<Arg>,
}

struct Arg {
    name: Option<Ident>,
    expr: Expr,
}

impl Parse for Input {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let template = input.parse()?;
        let mut args = Vec::new();
        if input.parse::<Option<Token![,]>>()?.is_some() {
            for arg in Punctuated::<Arg, Token![,]>::parse_terminated(input)? {
                args.push(arg);
            }
        }
        Ok(Input { template, args })
    }
}

impl Parse for Arg {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // `name = expr`, distinct from an expression such as `a == b`.
        if input.peek(Ident) && input.peek2(Token![=]) && !input.peek2(Token![==]) {
            let name = input.parse()?;
            input.parse::<Token![=]>()?;
            return Ok(Arg {
                name: Some(name),
                expr: input.parse()?,
            });
        }
        Ok(Arg {
            name: None,
            expr: input.parse()?,
        })
    }
}

enum Piece {
    Code(String),
    Arg(usize),
    Var(Ident),
}

pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let Input { template, args } = syn::parse2(input)?;
    let pieces = parse_template(&template, &args)?;

    let used: HashSet<usize> = pieces
        .iter()
        .filter_map(|p| match p {
            Piece::Arg(i) => Some(*i),
            _ => None,
        })
        .collect();
    if let Some(unused) = (0..args.len()).find(|i| !used.contains(i)) {
        return Err(syn::Error::new_spanned(
            &args[unused].expr,
            "argument never used in the `js!` template",
        ));
    }

    // Each argument is evaluated once, in order, however many times the
    // template uses it. `Span::mixed_site()` hides the bindings from the
    // user's expressions.
    let arg_names: Vec<Ident> = (0..args.len())
        .map(|i| format_ident!("__htmloxide_arg{}", i, span = Span::mixed_site()))
        .collect();
    let bindings = args.iter().zip(&arg_names).map(|(arg, name)| {
        let expr = &arg.expr;
        quote!(let #name = &(#expr);)
    });

    let code = Ident::new("__htmloxide_js", Span::mixed_site());
    let writes = pieces.iter().map(|piece| match piece {
        Piece::Code(text) => quote!(#code.push_str(#text);),
        Piece::Arg(i) => {
            let name = &arg_names[*i];
            quote!(::htmloxide::__private::js_part(#name, &mut #code);)
        }
        Piece::Var(var) => quote!(::htmloxide::__private::js_part(&#var, &mut #code);),
    });

    Ok(quote!({
        #( #bindings )*
        let mut #code = ::std::string::String::new();
        #( #writes )*
        ::htmloxide::__private::js_from_template(#code)
    }))
}

fn parse_template(template: &LitStr, args: &[Arg]) -> syn::Result<Vec<Piece>> {
    let error = |msg: &str| syn::Error::new(template.span(), msg);
    let text = template.value();
    let mut pieces = Vec::new();
    let mut code = String::new();
    let mut next_positional = 0;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                code.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                code.push('}');
            }
            '}' => {
                return Err(error(
                    "unmatched `}` in the `js!` template; write `}}` for a literal `}`",
                ));
            }
            '{' => {
                let mut inner = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) => inner.push(c),
                        None => {
                            return Err(error(
                                "unclosed `{` in the `js!` template; write `{{` for a literal `{`",
                            ));
                        }
                    }
                }
                if !code.is_empty() {
                    pieces.push(Piece::Code(std::mem::take(&mut code)));
                }
                pieces.push(placeholder(&inner, args, &mut next_positional, template)?);
            }
            c => code.push(c),
        }
    }
    if !code.is_empty() {
        pieces.push(Piece::Code(code));
    }
    Ok(pieces)
}

fn placeholder(
    inner: &str,
    args: &[Arg],
    next_positional: &mut usize,
    template: &LitStr,
) -> syn::Result<Piece> {
    let error = |msg: String| syn::Error::new(template.span(), msg);
    let positional = args.iter().take_while(|a| a.name.is_none()).count();

    if inner.is_empty() {
        let i = *next_positional;
        *next_positional += 1;
        return if i < positional {
            Ok(Piece::Arg(i))
        } else {
            Err(error(format!(
                "`{{}}` number {} in the `js!` template has no argument",
                i + 1
            )))
        };
    }
    if let Ok(i) = inner.parse::<usize>() {
        return if i < positional {
            Ok(Piece::Arg(i))
        } else {
            Err(error(format!(
                "`{{{i}}}` in the `js!` template has no argument"
            )))
        };
    }
    let Ok(name) = syn::parse_str::<Ident>(inner) else {
        return Err(error(format!(
            "`{{{inner}}}` in the `js!` template must be a name, like `{{id}}`; \
             write `{{{{` for a literal `{{`"
        )));
    };
    if let Some(i) = args
        .iter()
        .position(|a| a.name.as_ref().is_some_and(|n| *n == name))
    {
        return Ok(Piece::Arg(i));
    }
    // With the template's span, the name resolves in the caller's scope, and
    // an unknown name is reported at the template.
    let mut var = name;
    var.set_span(template.span());
    Ok(Piece::Var(var))
}
