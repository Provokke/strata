//! A small parser-combinator library over token slices.
//!
//! Parsers are any `Fn(Input) -> PResult<O>`. Errors carry spans and a
//! `committed` flag: once a parser has consumed enough to know it is on the
//! right path it `cut`s, and `alt` will no longer try alternatives. `recover`
//! records an error into the shared context and resynchronises, so a single
//! parse can report several errors.

use crate::lexer::{Tok, Token};
use crate::span::{ErrorKind, ShellError, Span};
use crate::types::Signature;
use std::cell::RefCell;
use std::collections::HashMap;

/// Shared parse context: collected (recovered) errors and the command table.
#[derive(Default)]
pub struct Ctx {
    pub errors: RefCell<Vec<ShellError>>,
    /// Nesting depth guard against stack overflow on adversarial input.
    pub depth: std::cell::Cell<usize>,
    pub sigs: RefCell<HashMap<String, Signature>>,
}

#[derive(Clone, Copy)]
pub struct Input<'a> {
    pub toks: &'a [Token],
    pub pos: usize,
    pub ctx: &'a Ctx,
}

impl<'a> Input<'a> {
    pub fn new(toks: &'a [Token], ctx: &'a Ctx) -> Self {
        Input { toks, pos: 0, ctx }
    }
    pub fn peek(&self) -> &'a Token {
        &self.toks[self.pos.min(self.toks.len() - 1)]
    }
    pub fn peek_at(&self, n: usize) -> &'a Token {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)]
    }
    pub fn advance(self) -> Self {
        Input { pos: (self.pos + 1).min(self.toks.len() - 1), ..self }
    }
    pub fn at_eof(&self) -> bool {
        matches!(self.peek().tok, Tok::Eof)
    }
    /// Span of the token before the current position.
    pub fn prev_span(&self) -> Span {
        if self.pos == 0 { self.peek().span } else { self.toks[self.pos - 1].span }
    }
    pub fn span_from(&self, start: Input<'a>) -> Span {
        let s = start.peek().span.start;
        let e = self.prev_span().end.max(s);
        Span::new(s, e)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PErr {
    pub span: Span,
    pub msg: String,
    pub label: String,
    pub committed: bool,
}

impl PErr {
    pub fn new(span: Span, msg: impl Into<String>, label: impl Into<String>) -> Self {
        PErr { span, msg: msg.into(), label: label.into(), committed: false }
    }
    pub fn expected(i: &Input, what: &str) -> Self {
        let t = i.peek();
        PErr::new(t.span, format!("expected {what}, found {}", t.tok.describe()), format!("expected {what}"))
    }
    pub fn into_shell(self) -> ShellError {
        ShellError::new(ErrorKind::Parse, self.msg, self.span, self.label)
    }
}

pub type PResult<'a, O> = Result<(Input<'a>, O), PErr>;

pub trait Parser<'a, O> {
    fn parse(&self, i: Input<'a>) -> PResult<'a, O>;
}

impl<'a, O, F> Parser<'a, O> for F
where
    F: Fn(Input<'a>) -> PResult<'a, O>,
{
    fn parse(&self, i: Input<'a>) -> PResult<'a, O> {
        self(i)
    }
}

/// Match a single token satisfying `f`, returning the mapped value.
pub fn token<'a, O>(what: &'static str, f: impl Fn(&'a Token) -> Option<O>) -> impl Parser<'a, O> {
    move |i: Input<'a>| match f(i.peek()) {
        Some(o) => Ok((i.advance(), o)),
        None => Err(PErr::expected(&i, what)),
    }
}

/// Match an exact token kind.
pub fn tok<'a>(t: Tok) -> impl Parser<'a, Span> {
    let what: &'static str = Box::leak(t.describe().into_boxed_str());
    token(what, move |x: &Token| if x.tok == t { Some(x.span) } else { None })
}

/// Match a bareword with exact text (keywords).
pub fn keyword<'a>(k: &'static str) -> impl Parser<'a, Span> {
    token(k, move |x: &Token| matches!(&x.tok, Tok::Word(w) if w == k).then_some(x.span))
}

pub fn map<'a, O, U>(p: impl Parser<'a, O>, f: impl Fn(O) -> U) -> impl Parser<'a, U> {
    move |i: Input<'a>| p.parse(i).map(|(r, o)| (r, f(o)))
}

/// Map with access to the span covered by the parser.
pub fn map_span<'a, O, U>(p: impl Parser<'a, O>, f: impl Fn(O, Span) -> U) -> impl Parser<'a, U> {
    move |i: Input<'a>| {
        let (r, o) = p.parse(i)?;
        let sp = r.span_from(i);
        Ok((r, f(o, sp)))
    }
}

/// Monadic bind: the output of `p` chooses the next parser.
pub fn and_then<'a, O, U, Q: Parser<'a, U>>(p: impl Parser<'a, O>, f: impl Fn(O) -> Q) -> impl Parser<'a, U> {
    move |i: Input<'a>| {
        let (r, o) = p.parse(i)?;
        f(o).parse(r)
    }
}

/// Validate/transform output, failing with a spanned error.
pub fn try_map<'a, O, U>(p: impl Parser<'a, O>, f: impl Fn(O, Span) -> Result<U, PErr>) -> impl Parser<'a, U> {
    move |i: Input<'a>| {
        let (r, o) = p.parse(i)?;
        let sp = r.span_from(i);
        f(o, sp).map(|u| (r, u))
    }
}

/// Try `a`, then `b` if `a` failed without committing. Keeps the furthest error.
pub fn alt<'a, O>(a: impl Parser<'a, O>, b: impl Parser<'a, O>) -> impl Parser<'a, O> {
    move |i: Input<'a>| match a.parse(i) {
        Ok(x) => Ok(x),
        Err(e) if e.committed => Err(e),
        Err(e1) => match b.parse(i) {
            Ok(x) => Ok(x),
            Err(e2) if e2.committed => Err(e2),
            Err(e2) => {
                if e1.span.start > e2.span.start {
                    Err(e1)
                } else if e2.span.start > e1.span.start {
                    Err(e2)
                } else {
                    let a = e1.label.trim_start_matches("expected ");
                    let b = e2.label.trim_start_matches("expected ");
                    let found = e1.msg.rsplit_once(", found ").map(|x| x.1.to_string()).unwrap_or_default();
                    Err(PErr::new(
                        e1.span,
                        format!("expected {a} or {b}, found {found}"),
                        format!("expected {a} or {b}"),
                    ))
                }
            }
        },
    }
}

pub fn opt<'a, O>(p: impl Parser<'a, O>) -> impl Parser<'a, Option<O>> {
    move |i: Input<'a>| match p.parse(i) {
        Ok((r, o)) => Ok((r, Some(o))),
        Err(e) if e.committed => Err(e),
        Err(_) => Ok((i, None)),
    }
}

/// Zero or more repetitions. Stops on a non-committed failure or no progress.
pub fn many<'a, O>(p: impl Parser<'a, O>) -> impl Parser<'a, Vec<O>> {
    move |mut i: Input<'a>| {
        let mut out = Vec::new();
        loop {
            match p.parse(i) {
                Ok((r, o)) => {
                    if r.pos == i.pos {
                        return Ok((r, out));
                    }
                    out.push(o);
                    i = r;
                }
                Err(e) if e.committed => return Err(e),
                Err(_) => return Ok((i, out)),
            }
        }
    }
}

pub fn many1<'a, O>(p: impl Parser<'a, O>) -> impl Parser<'a, Vec<O>> {
    let m = many(p);
    move |i: Input<'a>| {
        let (r, v) = m.parse(i)?;
        if v.is_empty() { Err(PErr::expected(&i, "at least one item")) } else { Ok((r, v)) }
    }
}

/// `p (sep p)*`, possibly empty, allowing a trailing separator.
pub fn sep_by<'a, O, S>(p: impl Parser<'a, O>, sep: impl Parser<'a, S>) -> impl Parser<'a, Vec<O>> {
    move |i: Input<'a>| {
        let mut out = Vec::new();
        let mut cur = i;
        loop {
            match p.parse(cur) {
                Ok((r, o)) => {
                    out.push(o);
                    cur = r;
                }
                Err(e) if e.committed => return Err(e),
                Err(_) => return Ok((cur, out)),
            }
            match sep.parse(cur) {
                Ok((r, _)) => cur = r,
                Err(_) => return Ok((cur, out)),
            }
        }
    }
}

pub fn pair<'a, A, B>(a: impl Parser<'a, A>, b: impl Parser<'a, B>) -> impl Parser<'a, (A, B)> {
    move |i: Input<'a>| {
        let (r, x) = a.parse(i)?;
        let (r, y) = b.parse(r)?;
        Ok((r, (x, y)))
    }
}

pub fn preceded<'a, A, B>(a: impl Parser<'a, A>, b: impl Parser<'a, B>) -> impl Parser<'a, B> {
    map(pair(a, b), |(_, y)| y)
}

pub fn terminated<'a, A, B>(a: impl Parser<'a, A>, b: impl Parser<'a, B>) -> impl Parser<'a, A> {
    map(pair(a, b), |(x, _)| x)
}

/// `open p close`; once `open` matched, a missing `close` is a committed error
/// pointing at the opening delimiter.
pub fn delimited<'a, O>(open: Tok, p: impl Parser<'a, O>, close: Tok) -> impl Parser<'a, O> {
    let o = tok(open);
    let closing = close.describe();
    move |i: Input<'a>| {
        let (r, open_span) = o.parse(i)?;
        let (r, x) = p.parse(r).map_err(|mut e| {
            e.committed = true;
            e
        })?;
        if r.peek().tok == close {
            Ok((r.advance(), x))
        } else {
            let found = r.peek();
            let mut e = PErr::new(
                open_span,
                format!("unclosed delimiter: expected {closing}, found {}", found.tok.describe()),
                "unclosed delimiter opened here",
            );
            e.committed = true;
            Err(e)
        }
    }
}

/// Mark errors from `p` as committed (no backtracking into alternatives).
pub fn cut<'a, O>(p: impl Parser<'a, O>) -> impl Parser<'a, O> {
    move |i: Input<'a>| {
        p.parse(i).map_err(|mut e| {
            e.committed = true;
            e
        })
    }
}

/// Left-associative operator chaining: `p (op p)*`.
pub fn chainl1<'a, O, Op>(
    p: impl Parser<'a, O>,
    op: impl Parser<'a, Op>,
    f: impl Fn(O, Op, O) -> O,
) -> impl Parser<'a, O> {
    move |i: Input<'a>| {
        let (mut cur, mut acc) = p.parse(i)?;
        loop {
            let Ok((r, o)) = op.parse(cur) else { return Ok((cur, acc)) };
            let (r, rhs) = cut(|x| p.parse(x)).parse(r)?;
            acc = f(acc, o, rhs);
            cur = r;
        }
    }
}

/// Error recovery: on failure, record the error in the context, skip tokens
/// until `sync` matches (without consuming it) and produce `fallback(span)`.
pub fn recover<'a, O>(
    p: impl Parser<'a, O>,
    sync: impl Fn(&Tok) -> bool,
    fallback: impl Fn(Span) -> O,
) -> impl Parser<'a, O> {
    move |i: Input<'a>| match p.parse(i) {
        Ok(x) => Ok(x),
        Err(e) => {
            i.ctx.errors.borrow_mut().push(e.into_shell());
            let mut r = i;
            let mut depth = 0i32;
            while !r.at_eof() {
                let t = &r.peek().tok;
                if depth == 0 && sync(t) {
                    break;
                }
                match t {
                    Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
                    Tok::RParen | Tok::RBracket | Tok::RBrace => {
                        if depth == 0 {
                            break;
                        }
                        depth -= 1
                    }
                    _ => {}
                }
                r = r.advance();
            }
            if r.pos == i.pos && !r.at_eof() && !sync(&r.peek().tok) {
                r = r.advance();
            }
            let sp = r.span_from(i);
            Ok((r, fallback(sp)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn int<'a>() -> impl Parser<'a, i64> {
        token("integer", |t: &Token| if let Tok::Int(i) = t.tok { Some(i) } else { None })
    }

    fn run<O>(src: &str, p: impl for<'a> Fn(Input<'a>) -> PResult<'a, O>) -> (Result<O, PErr>, usize, Vec<ShellError>) {
        let toks = lex(src).unwrap();
        let ctx = Ctx::default();
        let r = p(Input::new(&toks, &ctx));
        let errs = ctx.errors.borrow().clone();
        match r {
            Ok((rest, o)) => (Ok(o), rest.pos, errs),
            Err(e) => (Err(e), 0, errs),
        }
    }

    #[test]
    fn token_and_map() {
        let (r, pos, _) = run("42", |i| map(int(), |x| x * 2).parse(i));
        assert_eq!(r, Ok(84));
        assert_eq!(pos, 1);
        let (r, _, _) = run("x", |i| int().parse(i));
        let e = r.unwrap_err();
        assert_eq!(e.span, Span::new(0, 1));
        assert_eq!(e.msg, "expected integer, found 'x'");
    }

    #[test]
    fn alt_merges_expectations_at_same_position() {
        let (r, _, _) = run("x", |i| alt(int(), map(tok(Tok::Comma), |_| 0)).parse(i));
        assert_eq!(r.unwrap_err().msg, "expected integer or ',', found 'x'");
        let (r, _, _) = run(",", |i| alt(int(), map(tok(Tok::Comma), |_| 7)).parse(i));
        assert_eq!(r, Ok(7));
    }

    #[test]
    fn alt_respects_cut() {
        fn p(i: Input) -> PResult<i64> {
            alt(preceded(tok(Tok::LParen), cut(int())), map(tok(Tok::LParen), |_| -1)).parse(i)
        }
        let (r, _, _) = run("( x", p);
        let e = r.unwrap_err();
        assert!(e.committed);
        assert_eq!(e.span, Span::new(2, 3));
    }

    #[test]
    fn many_and_sep_by() {
        let (r, pos, _) = run("1 2 3 ,", |i| many(int()).parse(i));
        assert_eq!(r, Ok(vec![1, 2, 3]));
        assert_eq!(pos, 3);
        let (r, _, _) = run("1, 2, 3,", |i| sep_by(int(), tok(Tok::Comma)).parse(i));
        assert_eq!(r, Ok(vec![1, 2, 3]));
        let (r, _, _) = run("", |i| sep_by(int(), tok(Tok::Comma)).parse(i));
        assert_eq!(r, Ok(vec![]));
        let (r, _, _) = run("x", |i| many1(int()).parse(i));
        assert!(r.is_err());
    }

    #[test]
    fn and_then_binds() {
        // first int says how many more ints follow
        fn p(i: Input) -> PResult<Vec<i64>> {
            and_then(int(), |n| {
                move |i| {
                    let mut cur = i;
                    let mut v = vec![];
                    for _ in 0..n {
                        let (r, x) = int().parse(cur)?;
                        v.push(x);
                        cur = r;
                    }
                    Ok((cur, v))
                }
            })
            .parse(i)
        }
        let (r, pos, _) = run("2 5 6 7", p);
        assert_eq!(r, Ok(vec![5, 6]));
        assert_eq!(pos, 3);
    }

    #[test]
    fn delimited_reports_unclosed_open_span() {
        let (r, _, _) = run("[1 2", |i| delimited(Tok::LBracket, many(int()), Tok::RBracket).parse(i));
        let e = r.unwrap_err();
        assert_eq!(e.span, Span::new(0, 1));
        assert!(e.msg.starts_with("unclosed delimiter"));
    }

    #[test]
    fn chainl1_left_assoc() {
        fn p(i: Input) -> PResult<i64> {
            chainl1(int(), tok(Tok::Op("-")), |a, _, b| a - b).parse(i)
        }
        let (r, _, _) = run("10 - 3 - 2", p);
        assert_eq!(r, Ok(5));
        let (r, _, _) = run("10 - ,", p);
        let e = r.unwrap_err();
        assert!(e.committed);
        assert_eq!(e.span, Span::new(5, 6));
    }

    #[test]
    fn recover_collects_and_resyncs() {
        fn item(i: Input) -> PResult<i64> {
            recover(int(), |t| matches!(t, Tok::Comma), |_| -1).parse(i)
        }
        let (r, _, errs) = run("1, x y, 3", |i| sep_by(item, tok(Tok::Comma)).parse(i));
        assert_eq!(r, Ok(vec![1, -1, 3]));
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].span, Span::new(3, 4));
    }

    #[test]
    fn try_map_spans() {
        fn p(i: Input) -> PResult<i64> {
            try_map(int(), |n, sp| if n > 0 { Ok(n) } else { Err(PErr::new(sp, "neg", "neg")) }).parse(i)
        }
        let (r, _, _) = run("  -5", p);
        assert_eq!(r.unwrap_err().span, Span::new(2, 4));
    }
}
