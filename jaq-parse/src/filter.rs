use crate::{MathOp, OrdOp, Path, Spanned, Token};
use alloc::{boxed::Box, string::String, string::ToString, vec::Vec};
use chumsky::prelude::*;
use core::fmt;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug)]
pub enum AssignOp {
    Assign,
    Update,
    UpdateWith(MathOp),
}

impl fmt::Display for AssignOp {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Assign => "=".fmt(f),
            Self::Update => "|=".fmt(f),
            Self::UpdateWith(op) => write!(f, "{op}="),
        }
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug)]
pub enum BinaryOp {
    Pipe,
    Comma,
    Or,
    And,
    Math(MathOp),
    Assign(AssignOp),
    Ord(OrdOp),
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug)]
pub enum KeyVal {
    Filter(Spanned<Filter>, Spanned<Filter>),
    Str(String, Option<Spanned<Filter>>),
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug)]
pub enum Filter {
    Num(String),
    Str(String),
    Array(Option<Box<Spanned<Self>>>),
    Object(Vec<KeyVal>),
    Path(Path<Self>),
    If(Box<Spanned<Self>>, Box<Spanned<Self>>, Box<Spanned<Self>>),
    Call(String, Vec<Spanned<Self>>),
    Neg(Box<Spanned<Self>>),
    Binary(Box<Spanned<Self>>, BinaryOp, Box<Spanned<Self>>),
}

impl From<String> for Filter {
    fn from(s: String) -> Self {
        Self::Str(s)
    }
}

impl Filter {
    fn binary_with_span(a: Spanned<Self>, op: BinaryOp, b: Spanned<Self>) -> Spanned<Self> {
        let span = a.1.start..b.1.end;
        (Filter::Binary(Box::new(a), op, Box::new(b)), span)
    }
}

fn bin<P, O>(prev: P, op: O) -> impl Parser<Token, Spanned<Filter>, Error = P::Error> + Clone
where
    P: Parser<Token, Spanned<Filter>> + Clone,
    O: Parser<Token, BinaryOp, Error = P::Error> + Clone,
{
    let args = prev.clone().then(op.then(prev).repeated());
    args.foldl(|a, (op, b)| Filter::binary_with_span(a, op, b))
}

pub(crate) fn args<T, P>(arg: P) -> impl Parser<Token, Vec<T>, Error = P::Error> + Clone
where
    P: Parser<Token, T> + Clone,
{
    arg.separated_by(just(Token::Ctrl(';')))
        .delimited_by(just(Token::Ctrl('(')), just(Token::Ctrl(')')))
        .or_not()
        .map(Option::unwrap_or_default)
}

// 'Atoms' are filters that contain no ambiguity
fn atom<P>(filter: P, no_comma: P) -> impl Parser<Token, Spanned<Filter>, Error = P::Error> + Clone
where
    P: Parser<Token, Spanned<Filter>, Error = Simple<Token>> + Clone,
{
    let val = filter_map(|span, tok| match tok {
        Token::Num(n) => Ok(Filter::Num(n)),
        Token::Str(s) => Ok(Filter::Str(s)),
        _ => Err(Simple::expected_input_found(span, Vec::new(), Some(tok))),
    })
    .labelled("value");

    let ident = filter_map(|span, tok| match tok {
        Token::Ident(ident) => Ok(ident),
        _ => Err(Simple::expected_input_found(span, Vec::new(), Some(tok))),
    })
    .labelled("identifier");

    let key = filter_map(|span, tok| match tok {
        Token::Ident(s) | Token::Str(s) => Ok(s),
        _ => Err(Simple::expected_input_found(span, Vec::new(), Some(tok))),
    })
    .labelled("object key");

    // Atoms can also just be normal filters, but surrounded with parentheses
    let parenthesised = filter
        .clone()
        .delimited_by(just(Token::Ctrl('(')), just(Token::Ctrl(')')));

    let array = filter
        .clone()
        .or_not()
        .delimited_by(just(Token::Ctrl('[')), just(Token::Ctrl(']')))
        .map_with_span(|arr, span| (Filter::Array(arr.map(Box::new)), span));

    let is_val = just(Token::Ctrl(':')).ignore_then(no_comma);
    let key_str = key
        .then(is_val.clone().or_not())
        .map(|(key, val)| KeyVal::Str(key, val));
    let key_filter = parenthesised
        .clone()
        .then(is_val)
        .map(|(key, val)| KeyVal::Filter(key, val));
    let object = key_str
        .or(key_filter)
        .separated_by(just(Token::Ctrl(',')))
        .delimited_by(just(Token::Ctrl('{')), just(Token::Ctrl('}')))
        .collect();

    let object = object.map_with_span(|obj, span| (Filter::Object(obj), span));

    let path = crate::path::path(filter.clone());
    let path = path.map_with_span(|path, span| (Filter::Path(path), span));

    let if_ = just(Token::If).ignore_then(filter.clone().map(Box::new));
    let then = just(Token::Then).ignore_then(filter.clone().map(Box::new));
    let else_ = just(Token::Else).ignore_then(filter.clone().map(Box::new));
    let ite = if_.then(then).then(else_).then_ignore(just(Token::End));
    let ite = ite.map_with_span(|((if_, then), else_), span| (Filter::If(if_, then, else_), span));

    let call = ident.then(args(filter));
    let call = call.map_with_span(|(f, args), span| (Filter::Call(f, args), span));

    let delim = |open, close| (Token::Ctrl(open), Token::Ctrl(close));
    let strategy = |open, close, others| {
        nested_delimiters(Token::Ctrl(open), Token::Ctrl(close), others, |span| {
            (Filter::Path(Vec::new()), span)
        })
    };

    val.map_with_span(|filter, span| (filter, span))
        .or(parenthesised)
        .or(array)
        .or(object)
        .or(path)
        .or(ite)
        .or(call)
        .recover_with(strategy('(', ')', [delim('[', ']'), delim('{', '}')]))
        .recover_with(strategy('[', ']', [delim('{', '}'), delim('(', ')')]))
        .recover_with(strategy('{', '}', [delim('(', ')'), delim('[', ']')]))
}

fn math<P>(prev: P) -> impl Parser<Token, Spanned<Filter>, Error = Simple<Token>> + Clone
where
    P: Parser<Token, Spanned<Filter>, Error = Simple<Token>> + Clone,
{
    let neg = just(Token::Op("-".to_string()))
        .map_with_span(|_, span| span)
        .repeated()
        .then(prev)
        .foldr(|a, b| {
            let span = a.start..b.1.end;
            (Filter::Neg(Box::new(b)), span)
        });

    let math = |op: MathOp| just(Token::Op(op.to_string())).to(BinaryOp::Math(op));

    let rem = bin(neg, math(MathOp::Rem));
    // Product ops (multiply and divide) have equal precedence
    let mul_div = bin(rem, math(MathOp::Mul).or(math(MathOp::Div)));
    // Sum ops (add and subtract) have equal precedence
    bin(mul_div, math(MathOp::Add).or(math(MathOp::Sub)))
}

fn ord<P>(prev: P) -> impl Parser<Token, Spanned<Filter>, Error = P::Error> + Clone
where
    P: Parser<Token, Spanned<Filter>> + Clone,
{
    let ord = |op: OrdOp| just(Token::Op(op.to_string())).to(BinaryOp::Ord(op));

    let lt_gt = choice((
        ord(OrdOp::Lt),
        ord(OrdOp::Gt),
        ord(OrdOp::Le),
        ord(OrdOp::Ge),
    ));
    let lt_gt = bin(prev, lt_gt);
    // Comparison ops (equal, not-equal) have equal precedence
    bin(lt_gt, ord(OrdOp::Eq).or(ord(OrdOp::Ne)))
}

fn assign<P>(prev: P) -> impl Parser<Token, Spanned<Filter>, Error = P::Error> + Clone
where
    P: Parser<Token, Spanned<Filter>> + Clone,
{
    let assign = |op: AssignOp| just(Token::Op(op.to_string())).to(BinaryOp::Assign(op));

    let update_with = |op: MathOp| assign(AssignOp::UpdateWith(op));
    let assign = choice((
        assign(AssignOp::Assign),
        assign(AssignOp::Update),
        update_with(MathOp::Add),
        update_with(MathOp::Sub),
        update_with(MathOp::Mul),
        update_with(MathOp::Div),
        update_with(MathOp::Rem),
    ));

    let args = prev.clone().then(assign).repeated().then(prev);
    args.foldr(|(a, op), b| Filter::binary_with_span(a, op, b))
}

pub(crate) fn filter() -> impl Parser<Token, Spanned<Filter>, Error = Simple<Token>> + Clone {
    // filters that may or may not contain commas on the toplevel,
    // i.e. not inside parentheses
    let mut with_comma = Recursive::declare();
    let mut sans_comma = Recursive::declare();

    let atom = atom(with_comma.clone(), sans_comma.clone()).boxed();
    let math = math(atom).boxed();
    let ord = ord(math).boxed();
    let and = bin(ord, just(Token::And).to(BinaryOp::And));
    let or = bin(and, just(Token::Or).to(BinaryOp::Or));
    let assign = assign(or).boxed();

    let comma = just(Token::Ctrl(',')).to(BinaryOp::Comma);
    let pipe = just(Token::Op("|".to_string())).to(BinaryOp::Pipe);

    sans_comma.define(bin(assign.clone(), pipe.clone()));
    with_comma.define(bin(bin(assign, comma), pipe));

    with_comma
}
