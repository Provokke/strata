//! The strata grammar, built from the combinators in `combinator.rs`.

use crate::ast::*;
use crate::combinator::*;
use crate::lexer::{InterpPart, Tok, Token, lex, lex_at};
use crate::span::{ShellError, Span};
use crate::types::{FlagSpec, PosArg, Signature, Type};
use crate::value::Value;
use std::rc::Rc;

const MAX_DEPTH: usize = 96;

#[derive(Clone, Copy)]
struct Mode {
    /// Barewords are string literals (command arguments, list items).
    arg: bool,
    /// Barewords are column references (`where` row conditions).
    column: bool,
}

const EXPR: Mode = Mode { arg: false, column: false };
const ARG: Mode = Mode { arg: true, column: false };

/// Parse a source string. Returns the block (with `Stmt::Error` placeholders
/// where recovery happened) and all errors found.
pub fn parse(src: &str, ctx: &Ctx) -> (Block, Vec<ShellError>) {
    let toks = match lex(src) {
        Ok(t) => t,
        Err(e) => return (Block { stmts: vec![], span: Span::new(0, src.len()) }, vec![e]),
    };
    ctx.errors.borrow_mut().clear();
    ctx.depth.set(0);
    let block = parse_tokens(&toks, ctx);
    let errs = ctx.errors.borrow_mut().drain(..).collect();
    (block, errs)
}

fn parse_tokens(toks: &[Token], ctx: &Ctx) -> Block {
    let i = Input::new(toks, ctx);
    match block_body(i, |_| false) {
        Ok((r, b)) => {
            if !r.at_eof() {
                let t = r.peek();
                ctx.errors.borrow_mut().push(
                    PErr::new(t.span, format!("unexpected {}", t.tok.describe()), "unexpected token").into_shell(),
                );
            }
            b
        }
        Err(e) => {
            ctx.errors.borrow_mut().push(e.into_shell());
            Block { stmts: vec![], span: Span::unknown() }
        }
    }
}

fn is_sep(t: &Tok) -> bool {
    matches!(t, Tok::Newline | Tok::Semi)
}

fn is_closer(t: &Tok) -> bool {
    matches!(t, Tok::RParen | Tok::RBracket | Tok::RBrace)
}

fn skip_newlines(mut i: Input) -> Input {
    while matches!(i.peek().tok, Tok::Newline) {
        i = i.advance();
    }
    i
}

/// Statements separated by newlines/semicolons until `end` (not consumed).
fn block_body<'a>(i: Input<'a>, end: fn(&Tok) -> bool) -> PResult<'a, Block> {
    let start = i;
    let mut cur = i;
    let mut stmts = Vec::new();
    let sync = move |t: &Tok| is_sep(t) || end(t);
    loop {
        while is_sep(&cur.peek().tok) {
            cur = cur.advance();
        }
        if cur.at_eof() || end(&cur.peek().tok) || is_closer(&cur.peek().tok) {
            break;
        }
        let (r, s) = recover(stmt, sync, Stmt::Error).parse(cur)?;
        cur = r;
        stmts.push(s);
        let t = cur.peek();
        if !(is_sep(&t.tok) || t.tok == Tok::Eof || end(&t.tok) || is_closer(&t.tok)) {
            cur.ctx.errors.borrow_mut().push(
                PErr::new(t.span, format!("unexpected {}", t.tok.describe()), "expected end of statement").into_shell(),
            );
            while !(cur.at_eof() || sync(&cur.peek().tok) || is_closer(&cur.peek().tok)) {
                cur = cur.advance();
            }
        }
    }
    let span = cur.span_from(start);
    Ok((cur, Block { stmts, span }))
}

fn word<'a>() -> impl Parser<'a, (String, Span)> {
    token("name", |t: &Token| if let Tok::Word(w) = &t.tok { Some((w.clone(), t.span)) } else { None })
}

fn stmt(i: Input) -> PResult<Stmt> {
    let d = i.ctx.depth.get();
    if d > MAX_DEPTH {
        let mut e = PErr::new(i.peek().span, "nesting too deep", "nesting limit exceeded");
        e.committed = true;
        return Err(e);
    }
    i.ctx.depth.set(d + 1);
    let r = stmt_inner(i);
    i.ctx.depth.set(d);
    r
}

fn stmt_inner(i: Input) -> PResult<Stmt> {
    match &i.peek().tok {
        Tok::Word(w) if w == "let" || w == "mut" => let_stmt(i),
        Tok::Word(w) if w == "def" => def_stmt(i),
        Tok::Word(w) if w == "for" => for_stmt(i),
        Tok::Word(w) if w == "while" => while_stmt(i),
        Tok::Var(_, p) if p.is_empty() && matches!(i.peek_at(1).tok, Tok::Op("=" | "+=" | "-=" | "*=")) => {
            assign_stmt(i)
        }
        _ => map(pipeline, Stmt::Pipeline).parse(i),
    }
}

fn let_stmt(i: Input) -> PResult<Stmt> {
    let start = i;
    let (Tok::Word(kw), r) = (&i.peek().tok, i.advance()) else { unreachable!() };
    let mutable = kw == "mut";
    let name_p = alt(
        map(word(), |(w, _)| w),
        token("variable name", |t: &Token| match &t.tok {
            Tok::Var(n, p) if p.is_empty() => Some(n.clone()),
            _ => None,
        }),
    );
    let (r, name) = cut(name_p).parse(r)?;
    let (r, ty) = opt(preceded(tok(Tok::Colon), cut(type_name))).parse(r)?;
    let (r, _) = cut(tok(Tok::Op("="))).parse(r)?;
    let r = skip_newlines(r);
    let (r, value) = cut(pipeline).parse(r)?;
    let span = r.span_from(start);
    Ok((r, Stmt::Let { name, mutable, ty, value, span }))
}

fn type_name(i: Input) -> PResult<Type> {
    try_map(word(), |(w, sp), _| {
        Type::from_name(&w).ok_or_else(|| PErr::new(sp, format!("unknown type '{w}'"), "unknown type"))
    })
    .parse(i)
}

fn assign_stmt(i: Input) -> PResult<Stmt> {
    let start = i;
    let Tok::Var(name, _) = &i.peek().tok else { unreachable!() };
    let r = i.advance();
    let op = match r.peek().tok {
        Tok::Op("+=") => Some(BinOp::Add),
        Tok::Op("-=") => Some(BinOp::Sub),
        Tok::Op("*=") => Some(BinOp::Mul),
        _ => None,
    };
    let (r, value) = cut(pipeline).parse(r.advance())?;
    let span = r.span_from(start);
    Ok((r, Stmt::Assign { name: name.clone(), op, value, span }))
}

fn for_stmt(i: Input) -> PResult<Stmt> {
    let start = i;
    let r = i.advance();
    let (r, (var, _)) = cut(word()).parse(r)?;
    let (r, _) = cut(keyword("in")).parse(r)?;
    let (r, iter) = cut(|x| expr(x, EXPR)).parse(r)?;
    let (r, body) = cut(braced_block).parse(r)?;
    let span = r.span_from(start);
    Ok((r, Stmt::For { var, iter, body: Rc::new(body), span }))
}

fn while_stmt(i: Input) -> PResult<Stmt> {
    let start = i;
    let (r, cond) = cut(|x| expr(x, EXPR)).parse(i.advance())?;
    let (r, body) = cut(braced_block).parse(r)?;
    let span = r.span_from(start);
    Ok((r, Stmt::While { cond, body: Rc::new(body), span }))
}

fn braced_block(i: Input) -> PResult<Block> {
    delimited(Tok::LBrace, |x| block_body(x, |t| *t == Tok::RBrace), Tok::RBrace).parse(i)
}

fn literal_value(e: &Expr) -> Option<Value> {
    Some(match &e.kind {
        ExprKind::Int(i) => Value::int(*i, e.span),
        ExprKind::Float(f) => Value::float(*f, e.span),
        ExprKind::String(s) => Value::string(s.clone(), e.span),
        ExprKind::Bool(b) => Value::bool(*b, e.span),
        ExprKind::Nothing => Value::nothing(e.span),
        _ => return None,
    })
}

enum Param {
    Pos(PosArg),
    Rest(PosArg),
    Flag(FlagSpec),
}

fn param(i: Input) -> PResult<Param> {
    let ann = |r| opt(preceded(tok(Tok::Colon), cut(type_name))).parse(r);
    match &i.peek().tok {
        Tok::Word(w) => {
            let r = i.advance();
            let (r, ty) = ann(r)?;
            let ty = ty.unwrap_or(Type::Any);
            if let Some(rest) = w.strip_prefix("...") {
                return Ok((r, Param::Rest(PosArg { name: rest.into(), ty, optional: true, default: None })));
            }
            let (name, mut optional) = match w.strip_suffix('?') {
                Some(n) => (n.to_string(), true),
                None => (w.clone(), false),
            };
            let (r, default) = opt(preceded(tok(Tok::Op("=")), cut(|x| primary(x, ARG)))).parse(r)?;
            let default = match default {
                Some(e) => {
                    optional = true;
                    Some(literal_value(&e).ok_or_else(|| {
                        let mut er = PErr::new(e.span, "default value must be a literal", "not a literal");
                        er.committed = true;
                        er
                    })?)
                }
                None => None,
            };
            Ok((r, Param::Pos(PosArg { name, ty, optional, default })))
        }
        Tok::Flag(f) if !f.starts_with('-') => {
            let long = f.clone();
            let r = i.advance();
            let short_p = delimited(
                Tok::LParen,
                token("short flag", |t: &Token| match &t.tok {
                    Tok::Flag(s) if s.starts_with('-') => s.chars().nth(1),
                    _ => None,
                }),
                Tok::RParen,
            );
            let (r, short) = opt(short_p).parse(r)?;
            let (r, ty) = ann(r)?;
            Ok((r, Param::Flag(FlagSpec { long, short, arg: ty, desc: String::new() })))
        }
        _ => Err(PErr::expected(&i, "parameter")),
    }
}

fn def_stmt(i: Input) -> PResult<Stmt> {
    let start = i;
    let r = i.advance();
    let name_p = alt(
        map(word(), |(w, _)| w),
        token("command name", |t: &Token| if let Tok::Str(s) = &t.tok { Some(s.clone()) } else { None }),
    );
    let (r, name) = cut(name_p).parse(r)?;
    let sep = |x| map(many1(alt(tok(Tok::Comma), tok(Tok::Newline))), |_| ()).parse(x);
    let params_p = delimited(
        Tok::LBracket,
        |x| {
            let x = skip_newlines(x);
            let (r, v) = sep_by(param, sep).parse(x)?;
            Ok((skip_newlines(r), v))
        },
        Tok::RBracket,
    );
    let (r, params) = cut(params_p).parse(r)?;
    let mut sig = Signature::build(&name, "user-defined command");
    for p in params {
        match p {
            Param::Pos(a) => sig.positional.push(a),
            Param::Rest(a) => sig.rest = Some(a),
            Param::Flag(f) => sig.flags.push(f),
        }
    }
    // optional `: in -> out`
    let (r, io) = opt(preceded(tok(Tok::Colon), cut(pair(type_name, preceded(keyword("->"), type_name))))).parse(r)?;
    if let Some((a, b)) = io {
        sig.io.push((a, b));
    }
    // register before the body so recursion and later calls resolve
    i.ctx.sigs.borrow_mut().insert(name.clone(), sig.clone());
    let (r, body) = cut(braced_block).parse(r)?;
    let span = r.span_from(start);
    Ok((r, Stmt::Def(Rc::new(Def { sig, body: Rc::new(body), span }))))
}

fn pipe_sep(i: Input) -> PResult<()> {
    let r = skip_newlines(i);
    let (r, _) = tok(Tok::Pipe).parse(r)?;
    Ok((skip_newlines(r), ()))
}

fn pipeline(i: Input) -> PResult<Pipeline> {
    let start = i;
    let (r, first) = element(i)?;
    let (r, rest) = many(preceded(pipe_sep, cut(element))).parse(r)?;
    let mut elements = vec![first];
    elements.extend(rest);
    let span = r.span_from(start);
    Ok((r, Pipeline { elements, span }))
}

fn element(i: Input) -> PResult<Expr> {
    match &i.peek().tok {
        Tok::Caret => external(i.advance(), i),
        Tok::Word(w) if !matches!(w.as_str(), "true" | "false" | "null" | "if" | "not") => call(i),
        _ => expr(i, EXPR),
    }
}

fn at_arg_end(t: &Tok) -> bool {
    matches!(t, Tok::Pipe | Tok::Newline | Tok::Semi | Tok::Eof | Tok::RParen | Tok::RBrace | Tok::RBracket)
}

fn external<'a>(i: Input<'a>, start: Input<'a>) -> PResult<'a, Expr> {
    let (mut r, head) = cut(|x| primary(x, ARG)).parse(i)?;
    let mut args = Vec::new();
    while !at_arg_end(&r.peek().tok) {
        let t = r.peek();
        if let Tok::Flag(f) = &t.tok {
            let s = if f.starts_with('-') { f.clone() } else { format!("--{f}") };
            args.push(Expr::new(ExprKind::String(s), t.span));
            r = r.advance();
            continue;
        }
        let (n, a) = cut(|x| primary(x, ARG)).parse(r)?;
        args.push(a);
        r = n;
    }
    let span = r.span_from(start);
    Ok((r, Expr::new(ExprKind::External { head: Box::new(head), args }, span)))
}

fn call(i: Input) -> PResult<Expr> {
    let Tok::Word(w) = &i.peek().tok else { unreachable!() };
    let name_span = i.peek().span;
    let sig = {
        let sigs = i.ctx.sigs.borrow();
        let two = match &i.peek_at(1).tok {
            Tok::Word(w2) => Some(format!("{w} {w2}")),
            _ => None,
        };
        match two.as_ref().and_then(|n| sigs.get(n)) {
            Some(s) => Some((s.clone(), 2)),
            None => sigs.get(w.as_str()).map(|s| (s.clone(), 1)),
        }
    };
    let Some((sig, nwords)) = sig else {
        return external(i, i);
    };
    let name_span = if nwords == 2 { name_span.merge(i.peek_at(1).span) } else { name_span };
    let mut r = i.advance();
    if nwords == 2 {
        r = r.advance();
    }
    let mut args = Vec::new();
    let mut npos = 0;
    while !at_arg_end(&r.peek().tok) {
        let t = r.peek();
        if let Tok::Flag(f) = &t.tok {
            let Some(spec) = sig.find_flag(f) else {
                let shown = if f.starts_with('-') { f.clone() } else { format!("--{f}") };
                let mut e =
                    PErr::new(t.span, format!("unknown flag {shown} for command '{}'", sig.name), "unknown flag");
                e.committed = true;
                return Err(e);
            };
            r = r.advance();
            let value = if spec.arg.is_some() {
                if at_arg_end(&r.peek().tok) {
                    let mut e = PErr::new(t.span, format!("flag --{} requires a value", spec.long), "missing value");
                    e.committed = true;
                    return Err(e);
                }
                let (n, v) = cut(|x| primary(x, ARG)).parse(r)?;
                r = n;
                Some(v)
            } else {
                None
            };
            args.push(Arg::Named { name: spec.long.clone(), span: t.span, value });
            continue;
        }
        let is_cond = sig.positional.get(npos).map(|p| p.ty == Type::RowCondition).unwrap_or(false);
        let (n, a) = if is_cond { cut(row_condition).parse(r)? } else { cut(|x| primary(x, ARG)).parse(r)? };
        args.push(Arg::Positional(a));
        npos += 1;
        r = n;
    }
    let span = r.span_from(i);
    Ok((r, Expr::new(ExprKind::Call(Box::new(Call { name: sig.name.clone(), name_span, args })), span)))
}

/// `where size > 10` — an expression where barewords name columns.
fn row_condition(i: Input) -> PResult<Expr> {
    let (r, e) = expr(i, Mode { arg: false, column: true })?;
    if matches!(e.kind, ExprKind::Closure { .. }) {
        return Ok((r, e));
    }
    let span = e.span;
    let body = Block { stmts: vec![Stmt::Pipeline(Pipeline { elements: vec![e], span })], span };
    Ok((r, Expr::new(ExprKind::Closure { params: vec![], body: Rc::new(body) }, span)))
}

// ---------------------------------------------------------------------------
// Expressions

fn binop<'a>(ops: &'static [&'static str]) -> impl Parser<'a, BinOp> {
    token("operator", move |t: &Token| {
        let s = match &t.tok {
            Tok::Op(o) => *o,
            Tok::Word(w) => w.as_str(),
            _ => return None,
        };
        if ops.contains(&s) { BinOp::parse_op(s) } else { None }
    })
}

fn mk_bin(l: Expr, op: BinOp, r: Expr) -> Expr {
    let span = l.span.merge(r.span);
    Expr::new(ExprKind::Binary { op, lhs: Box::new(l), rhs: Box::new(r) }, span)
}

fn expr(i: Input, m: Mode) -> PResult<Expr> {
    let d = i.ctx.depth.get();
    if d > MAX_DEPTH {
        let mut e = PErr::new(i.peek().span, "nesting too deep", "nesting limit exceeded");
        e.committed = true;
        return Err(e);
    }
    i.ctx.depth.set(d + 1);
    let r = chainl1(|x| and_expr(x, m), binop(&["or"]), mk_bin).parse(i);
    i.ctx.depth.set(d);
    r
}

fn and_expr(i: Input, m: Mode) -> PResult<Expr> {
    chainl1(|x| not_expr(x, m), binop(&["and"]), mk_bin).parse(i)
}

fn not_expr(i: Input, m: Mode) -> PResult<Expr> {
    if let Ok((r, sp)) = keyword("not").parse(i) {
        let (r, e) = cut(|x| not_expr(x, m)).parse(r)?;
        let span = sp.merge(e.span);
        return Ok((r, Expr::new(ExprKind::Unary { op: UnOp::Not, expr: Box::new(e) }, span)));
    }
    cmp_expr(i, m)
}

fn cmp_expr(i: Input, m: Mode) -> PResult<Expr> {
    let ops = &["==", "!=", "<", "<=", ">", ">=", "=~", "!~", "in", "not-in", "starts-with", "ends-with"];
    chainl1(
        |x| add_expr(x, m),
        binop(ops),
        move |l, op, mut r| {
            // `where type == file`: a bare right-hand side is a string, not a column
            if m.column
                && matches!(l.kind, ExprKind::Column(_))
                && let ExprKind::Column(c) = &r.kind
            {
                r.kind = ExprKind::String(c.clone());
            }
            mk_bin(l, op, r)
        },
    )
    .parse(i)
}

fn add_expr(i: Input, m: Mode) -> PResult<Expr> {
    chainl1(|x| mul_expr(x, m), binop(&["+", "-", "++"]), mk_bin).parse(i)
}

fn mul_expr(i: Input, m: Mode) -> PResult<Expr> {
    chainl1(|x| pow_expr(x, m), binop(&["*", "/", "//", "mod"]), mk_bin).parse(i)
}

fn pow_expr(i: Input, m: Mode) -> PResult<Expr> {
    let (r, base) = unary(i, m)?;
    if let Ok((r2, op)) = binop(&["**"]).parse(r) {
        let (r3, exp) = cut(|x| pow_expr(x, m)).parse(r2)?;
        return Ok((r3, mk_bin(base, op, exp)));
    }
    Ok((r, base))
}

fn unary(i: Input, m: Mode) -> PResult<Expr> {
    if let Ok((r, sp)) = tok(Tok::Op("-")).parse(i) {
        let (r, e) = cut(|x| unary(x, m)).parse(r)?;
        let span = sp.merge(e.span);
        return Ok((r, Expr::new(ExprKind::Unary { op: UnOp::Neg, expr: Box::new(e) }, span)));
    }
    range_expr(i, m)
}

fn range_expr(i: Input, m: Mode) -> PResult<Expr> {
    let (r, from) = primary(i, m)?;
    let op = match r.peek().tok {
        Tok::DotDot => true,
        Tok::DotDotLt => false,
        _ => return Ok((r, from)),
    };
    let r = r.advance();
    let (r, to) = if matches!(r.peek().tok, Tok::Int(_) | Tok::Var(..) | Tok::LParen) {
        let (r, t) = primary(r, m)?;
        (r, Some(Box::new(t)))
    } else {
        (r, None)
    };
    let span = r.span_from(i);
    Ok((r, Expr::new(ExprKind::Range { from: Box::new(from), to, inclusive: op }, span)))
}

/// Atoms. Exposed for command arguments (`m.arg`), which do not take infix operators.
fn primary(i: Input, m: Mode) -> PResult<Expr> {
    let (r, e) = primary_inner(i, m)?;
    // ranges are allowed directly in argument position: `range 1..10`
    if m.arg && matches!(r.peek().tok, Tok::DotDot | Tok::DotDotLt) {
        return range_expr(i, Mode { arg: false, ..m });
    }
    Ok((r, e))
}

fn primary_inner(i: Input, m: Mode) -> PResult<Expr> {
    let t = i.peek();
    let sp = t.span;
    let lit = |k| Ok((i.advance(), Expr::new(k, sp)));
    match &t.tok {
        Tok::Int(n) => lit(ExprKind::Int(*n)),
        Tok::Float(f) => lit(ExprKind::Float(*f)),
        Tok::Filesize(n) => lit(ExprKind::Filesize(*n)),
        Tok::Duration(n) => lit(ExprKind::Duration(*n)),
        Tok::Str(s) => lit(ExprKind::String(s.clone())),
        Tok::Interp(parts) => interp(i, parts),
        Tok::Var(name, path) => {
            let v = Expr::new(ExprKind::Var(name.clone()), Span::new(sp.start, sp.start + 1 + name.len()));
            if path.is_empty() { lit(v.kind) } else { lit(ExprKind::Path { head: Box::new(v), path: path.clone() }) }
        }
        Tok::LParen => {
            let (r, b) = delimited(Tok::LParen, |x| block_body(x, |t| *t == Tok::RParen), Tok::RParen).parse(i)?;
            let span = r.span_from(i);
            let e = Expr::new(ExprKind::Subexpr(Rc::new(b)), span);
            if let Tok::PathTail(path) = &r.peek().tok {
                let r2 = r.advance();
                let span = r2.span_from(i);
                return Ok((r2, Expr::new(ExprKind::Path { head: Box::new(e), path: path.clone() }, span)));
            }
            Ok((r, e))
        }
        Tok::LBracket => {
            let item_sep = |x| map(many1(alt(tok(Tok::Comma), tok(Tok::Newline))), |_| ()).parse(x);
            let items = |x| {
                let x = skip_newlines(x);
                let (r, v) = sep_by(|y| expr(y, ARG), alt(item_sep, map(|z| Ok((z, ())), |_| ()))).parse(x)?;
                Ok((skip_newlines(r), v))
            };
            let (r, v) = delimited(Tok::LBracket, items, Tok::RBracket).parse(i)?;
            let span = r.span_from(i);
            Ok((r, Expr::new(ExprKind::List(v), span)))
        }
        Tok::LBrace => brace(i),
        Tok::Word(w) => match w.as_str() {
            "true" => lit(ExprKind::Bool(true)),
            "false" => lit(ExprKind::Bool(false)),
            "null" => lit(ExprKind::Nothing),
            "if" => if_expr(i),
            _ if m.column => lit(ExprKind::Column(w.clone())),
            _ if m.arg => lit(ExprKind::String(w.clone())),
            _ => Err(PErr::new(
                sp,
                format!("unexpected bareword '{w}' in expression"),
                "bareword not allowed here; quote it or wrap a command in ( )",
            )),
        },
        _ => Err(PErr::expected(&i, "expression")),
    }
}

fn interp<'a>(i: Input<'a>, parts: &[InterpPart]) -> PResult<'a, Expr> {
    let sp = i.peek().span;
    let mut out = Vec::new();
    for p in parts {
        match p {
            InterpPart::Lit(s) => out.push(Expr::new(ExprKind::String(s.clone()), sp)),
            InterpPart::Expr(src, off) => {
                let toks = lex_at(src, *off).map_err(|e| {
                    let mut pe = PErr::new(e.span, e.msg, e.label);
                    pe.committed = true;
                    pe
                })?;
                let inner = Input::new(&toks, i.ctx);
                let (r, b) = block_body(inner, |_| false)?;
                if !r.at_eof() {
                    let mut e =
                        PErr::new(r.peek().span, format!("unexpected {}", r.peek().tok.describe()), "unexpected");
                    e.committed = true;
                    return Err(e);
                }
                let span = Span::new(*off, off + src.len());
                out.push(Expr::new(ExprKind::Subexpr(Rc::new(b)), span));
            }
        }
    }
    Ok((i.advance(), Expr::new(ExprKind::Interp(out), sp)))
}

fn if_expr(i: Input) -> PResult<Expr> {
    let start = i;
    let r = i.advance();
    let (r, cond) = cut(|x| expr(x, EXPR)).parse(r)?;
    let (r, then) = cut(braced_block).parse(r)?;
    let look = skip_newlines(r);
    let (r, else_) = if let Ok((r2, _)) = keyword("else").parse(look) {
        if matches!(&r2.peek().tok, Tok::Word(w) if w == "if") {
            let (r3, e) = if_expr(r2)?;
            (r3, Some(Box::new(e)))
        } else {
            let (r3, b) = cut(braced_block).parse(r2)?;
            let span = r3.span_from(r2);
            (r3, Some(Box::new(Expr::new(ExprKind::Block(Rc::new(b)), span))))
        }
    } else {
        (r, None)
    };
    let span = r.span_from(start);
    Ok((r, Expr::new(ExprKind::If { cond: Box::new(cond), then: Rc::new(then), else_ }, span)))
}

fn brace(i: Input) -> PResult<Expr> {
    let next = skip_newlines(i.advance());
    let is_record = matches!(next.peek().tok, Tok::RBrace)
        || (matches!(next.peek().tok, Tok::Word(_) | Tok::Str(_)) && next.peek_at(1).tok == Tok::Colon);
    if is_record {
        let field = pair(terminated(record_key, cut(tok(Tok::Colon))), cut(|x| expr(skip_newlines(x), ARG)));
        let sep = |x| map(many1(alt(tok(Tok::Comma), tok(Tok::Newline))), |_| ()).parse(x);
        let fields = |x| {
            let x = skip_newlines(x);
            let (r, v) = sep_by(|y| field.parse(y), alt(sep, |z| Ok((z, ())))).parse(x)?;
            Ok((skip_newlines(r), v))
        };
        let (r, v) = delimited(Tok::LBrace, fields, Tok::RBrace).parse(i)?;
        let span = r.span_from(i);
        return Ok((r, Expr::new(ExprKind::Record(v), span)));
    }
    // closure: `{|a, b| ...}` or `{ ... }`
    let (r, (params, body)) = delimited(Tok::LBrace, closure_body, Tok::RBrace).parse(i)?;
    let span = r.span_from(i);
    Ok((r, Expr::new(ExprKind::Closure { params, body: Rc::new(body) }, span)))
}

fn record_key(x: Input) -> PResult<Expr> {
    let t = x.peek();
    match &t.tok {
        Tok::Word(w) | Tok::Str(w) => Ok((x.advance(), Expr::new(ExprKind::String(w.clone()), t.span))),
        _ => Err(PErr::expected(&x, "record key")),
    }
}

fn closure_body(x: Input) -> PResult<(Vec<String>, Block)> {
    let (r, params) = if x.peek().tok == Tok::Pipe {
        let names = sep_by(map(word(), |(w, _)| w), tok(Tok::Comma));
        delimited(Tok::Pipe, names, Tok::Pipe).parse(x)?
    } else {
        (x, vec![])
    };
    let (r, b) = block_body(r, |t| *t == Tok::RBrace)?;
    Ok((r, (params, b)))
}
