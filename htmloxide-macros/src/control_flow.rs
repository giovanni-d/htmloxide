//! `@if`, `@for`, `@match` and `@let`: Rust control flow inside markup.
//!
//! ```ignore
//! @for item in &items {
//!     @if item.done { <li class="done">{item.title}</li> } else { <li>{item.title}</li> }
//! }
//! ```
//!
//! `ControlFlow` is the user-defined node type that rstml accepts. At every
//! position where a node can start, rstml calls `ControlFlow::peek_element`; on `@`
//! followed by one of the keywords, it calls `ControlFlow::parse_recoverable`. The
//! Rust parts (conditions, patterns, iterators) are parsed with syn, and each
//! `{..}` body is parsed by rstml as markup. The generator emits each node as
//! the Rust construct it names, around the code of its bodies.

use std::fmt;

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::ToTokens;
use rstml::node::{CustomNode, Node};
use rstml::recoverable::{ParseRecoverable, RecoverableContext};
use syn::parse::ParseStream;
use syn::token::Brace;
use syn::{Expr, Pat, Stmt, Token, braced};

#[derive(Clone)]
pub enum ControlFlow {
    If(ControlFlowIf),
    For(ControlFlowFor),
    Match(ControlFlowMatch),
    Let(ControlFlowLet),
    /// A construct that failed to parse. Its error is already reported, and
    /// it writes nothing.
    Invalid(TokenStream),
}

#[derive(Clone)]
pub struct ControlFlowIf {
    pub at: Token![@],
    pub branches: Vec<Branch>,
}

#[derive(Clone)]
pub struct Branch {
    pub else_token: Option<Token![else]>,
    pub cond: Option<(Token![if], Expr)>,
    pub body: Body,
}

#[derive(Clone)]
pub struct ControlFlowFor {
    pub at: Token![@],
    pub for_token: Token![for],
    pub pat: Pat,
    pub in_token: Token![in],
    pub expr: Expr,
    pub body: Body,
}

#[derive(Clone)]
pub struct ControlFlowMatch {
    pub at: Token![@],
    pub match_token: Token![match],
    pub expr: Expr,
    pub brace: Brace,
    pub arms: Vec<Arm>,
}

#[derive(Clone)]
pub struct Arm {
    pub pat: Pat,
    pub guard: Option<(Token![if], Expr)>,
    pub arrow: Token![=>],
    pub body: Body,
    pub comma: Option<Token![,]>,
}

#[derive(Clone)]
pub struct ControlFlowLet {
    pub at: Token![@],
    pub stmt: Stmt,
}

#[derive(Clone)]
pub struct Body {
    pub brace: Brace,
    pub nodes: Vec<Node<ControlFlow>>,
}

impl CustomNode for ControlFlow {
    fn peek_element(input: ParseStream) -> bool {
        // `@else` is accepted only so that `ControlFlow::parse` can explain it.
        input.peek(Token![@])
            && (input.peek2(Token![if])
                || input.peek2(Token![for])
                || input.peek2(Token![match])
                || input.peek2(Token![let])
                || input.peek2(Token![else]))
    }
}

impl ParseRecoverable for ControlFlow {
    // A construct that fails to parse is reported and skipped, so the rest of
    // the markup still parses.
    //
    // It is parsed on a fork, and the input advances only on success. When a
    // syn parse fails inside a `{..}` group, the tokens left in the group are
    // recorded as unexpected and reported once the whole parse ends, which
    // rstml turns into a panic. A fork discards that record.
    fn parse_recoverable(parser: &mut RecoverableContext, input: ParseStream) -> Option<Self> {
        use syn::parse::discouraged::Speculative;
        let fork = input.fork();
        match Self::parse(parser, &fork) {
            Ok(control_flow) => {
                input.advance_to(&fork);
                Some(control_flow)
            }
            Err(error) => {
                parser.push_diagnostic(error);
                Some(ControlFlow::Invalid(skip_construct(input)))
            }
        }
    }
}

impl ControlFlow {
    fn parse(parser: &mut RecoverableContext, input: ParseStream) -> syn::Result<Self> {
        let at: Token![@] = input.parse()?;
        if input.peek(Token![else]) {
            return Err(syn::Error::new(
                at.span,
                "`else` follows the `@if` body without `@`: `@if a { .. } else { .. }`",
            ));
        }
        let lookahead = input.lookahead1();
        if lookahead.peek(Token![if]) {
            ControlFlowIf::parse(at, parser, input).map(ControlFlow::If)
        } else if lookahead.peek(Token![for]) {
            let for_token: Token![for] = input.parse()?;
            // Without `in`, syn parses `item { .. }` as a struct pattern and
            // reports errors about the markup inside it.
            let pat_in = || -> syn::Result<(Pat, Token![in])> {
                Ok((Pat::parse_multi_with_leading_vert(input)?, input.parse()?))
            };
            let (pat, in_token) = pat_in().map_err(|_| {
                syn::Error::new(for_token.span, "expected `@for pattern in iterator { .. }`")
            })?;
            Ok(ControlFlow::For(ControlFlowFor {
                at,
                for_token,
                pat,
                in_token,
                expr: Expr::parse_without_eager_brace(input)?,
                body: Body::parse(parser, input)?,
            }))
        } else if lookahead.peek(Token![match]) {
            ControlFlowMatch::parse(at, parser, input).map(ControlFlow::Match)
        } else if lookahead.peek(Token![let]) {
            Ok(ControlFlow::Let(ControlFlowLet {
                at,
                stmt: input.parse()?,
            }))
        } else {
            Err(lookahead.error())
        }
    }
}

impl ControlFlowIf {
    fn parse(
        at: Token![@],
        parser: &mut RecoverableContext,
        input: ParseStream,
    ) -> syn::Result<Self> {
        // `parse_without_eager_brace` stops before `{`, as Rust does after
        // `if`. Otherwise `cond { .. }` would be parsed as a struct literal,
        // with the body as its fields.
        let mut branches = vec![Branch {
            else_token: None,
            cond: Some((input.parse()?, Expr::parse_without_eager_brace(input)?)),
            body: Body::parse(parser, input)?,
        }];
        while input.peek(Token![else]) {
            let else_token = Some(input.parse()?);
            let cond = if input.peek(Token![if]) {
                Some((input.parse()?, Expr::parse_without_eager_brace(input)?))
            } else {
                None
            };
            let is_last = cond.is_none();
            branches.push(Branch {
                else_token,
                cond,
                body: Body::parse(parser, input)?,
            });
            if is_last {
                break;
            }
        }
        Ok(ControlFlowIf { at, branches })
    }
}

impl ControlFlowMatch {
    fn parse(
        at: Token![@],
        parser: &mut RecoverableContext,
        input: ParseStream,
    ) -> syn::Result<Self> {
        let match_token = input.parse()?;
        let expr = Expr::parse_without_eager_brace(input)?;
        let content;
        let brace = braced!(content in input);
        let arms = Arm::parse_all(parser, &content)?;
        Ok(ControlFlowMatch {
            at,
            match_token,
            expr,
            brace,
            arms,
        })
    }
}

impl Arm {
    fn parse_all(parser: &mut RecoverableContext, input: ParseStream) -> syn::Result<Vec<Self>> {
        let mut arms = Vec::new();
        while !input.is_empty() {
            let pat = Pat::parse_multi_with_leading_vert(input)?;
            let guard = if input.peek(Token![if]) {
                Some((input.parse()?, input.parse()?))
            } else {
                None
            };
            let arrow = input.parse()?;
            if !input.peek(Brace) {
                return Err(input.error("an `@match` arm's markup goes in braces: `pat => { .. }`"));
            }
            arms.push(Arm {
                pat,
                guard,
                arrow,
                body: Body::parse(parser, input)?,
                comma: input.parse()?,
            });
        }
        Ok(arms)
    }
}

impl Body {
    fn parse(parser: &mut RecoverableContext, input: ParseStream) -> syn::Result<Self> {
        let content;
        let brace = braced!(content in input);
        let mut nodes = Vec::new();
        while !content.is_empty() {
            let Some(node) = parser.parse_recoverable::<Node<ControlFlow>>(&content) else {
                // rstml has reported the error. The rest of the body is
                // consumed so that it is not reported again as unexpected.
                let _ = content.parse::<TokenStream>();
                break;
            };
            nodes.push(node);
        }
        Ok(Body { brace, nodes })
    }
}

/// Skips a construct that failed to parse: through its first body and any
/// `else` branches, stopping early at the `<` of the next tag.
fn skip_construct(input: ParseStream) -> TokenStream {
    let mut skipped = TokenStream::new();
    while !input.is_empty() && !input.peek(Token![<]) {
        let Ok(tt) = input.parse::<TokenTree>() else {
            break;
        };
        let is_body = matches!(&tt, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace);
        skipped.extend([tt]);
        if is_body && !input.peek(Token![else]) {
            break;
        }
    }
    skipped
}

// The generator reads a node's tokens to find where it starts and ends in
// the source, which decides the whitespace around it.
impl ToTokens for ControlFlow {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        match self {
            ControlFlow::If(f) => f.to_tokens(tokens),
            ControlFlow::For(f) => f.to_tokens(tokens),
            ControlFlow::Match(f) => f.to_tokens(tokens),
            ControlFlow::Let(f) => f.to_tokens(tokens),
            ControlFlow::Invalid(skipped) => skipped.to_tokens(tokens),
        }
    }
}

impl ToTokens for ControlFlowIf {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.at.to_tokens(tokens);
        for branch in &self.branches {
            branch.else_token.to_tokens(tokens);
            if let Some((if_token, cond)) = &branch.cond {
                if_token.to_tokens(tokens);
                cond.to_tokens(tokens);
            }
            branch.body.to_tokens(tokens);
        }
    }
}

impl ToTokens for ControlFlowFor {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.at.to_tokens(tokens);
        self.for_token.to_tokens(tokens);
        self.pat.to_tokens(tokens);
        self.in_token.to_tokens(tokens);
        self.expr.to_tokens(tokens);
        self.body.to_tokens(tokens);
    }
}

impl ToTokens for ControlFlowMatch {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.at.to_tokens(tokens);
        self.match_token.to_tokens(tokens);
        self.expr.to_tokens(tokens);
        self.brace.surround(tokens, |tokens| {
            for arm in &self.arms {
                arm.pat.to_tokens(tokens);
                if let Some((if_token, guard)) = &arm.guard {
                    if_token.to_tokens(tokens);
                    guard.to_tokens(tokens);
                }
                arm.arrow.to_tokens(tokens);
                arm.body.to_tokens(tokens);
                arm.comma.to_tokens(tokens);
            }
        });
    }
}

impl ToTokens for ControlFlowLet {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.at.to_tokens(tokens);
        self.stmt.to_tokens(tokens);
    }
}

impl ToTokens for Body {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.brace.surround(tokens, |tokens| {
            for node in &self.nodes {
                node.to_tokens(tokens);
            }
        });
    }
}

// Required by rstml. syn's syntax tree implements `Debug` only with its
// `extra-traits` feature, so only the keyword is printed.
impl fmt::Debug for ControlFlow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ControlFlow::If(_) => "@if",
            ControlFlow::For(_) => "@for",
            ControlFlow::Match(_) => "@match",
            ControlFlow::Let(_) => "@let",
            ControlFlow::Invalid(_) => "invalid control flow",
        })
    }
}
