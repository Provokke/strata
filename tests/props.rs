//! Property tests: printer/parser round trip and parser/checker fuzzing.

use proptest::prelude::*;
use std::rc::Rc;
use strata::ast::*;
use strata::combinator::Ctx;
use strata::engine::Engine;
use strata::parser::parse;
use strata::span::Span;

fn sp() -> Span {
    Span::unknown()
}

fn e(kind: ExprKind) -> Expr {
    Expr::new(kind, sp())
}

/// Compound operands are wrapped in `( )`, exactly as the parser represents them.
fn group(x: Expr) -> Expr {
    match x.kind {
        ExprKind::Binary { .. } | ExprKind::Unary { .. } => {
            let block = Block { stmts: vec![Stmt::Pipeline(Pipeline { elements: vec![x], span: sp() })], span: sp() };
            e(ExprKind::Subexpr(Rc::new(block)))
        }
        _ => x,
    }
}

fn ident() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,6}"
}

fn leaf() -> impl Strategy<Value = Expr> {
    prop_oneof![
        any::<i64>().prop_map(|i| e(ExprKind::Int(i))),
        (-4000i32..4000).prop_map(|i| e(ExprKind::Float(i as f64 / 4.0))),
        any::<bool>().prop_map(|b| e(ExprKind::Bool(b))),
        Just(e(ExprKind::Nothing)),
        any::<String>().prop_map(|s| e(ExprKind::String(s.replace('\r', "")))),
        ident().prop_map(|v| e(ExprKind::Var(v))),
        (0i64..1_000_000).prop_map(|b| e(ExprKind::Filesize(b))),
        (ident(), prop::collection::vec("[a-z][a-z0-9]{0,4}", 1..3)).prop_map(|(v, path)| {
            let head = Box::new(e(ExprKind::Var(v)));
            let path = path.into_iter().map(|p| strata::value::PathMember::parse(&p, sp())).collect();
            e(ExprKind::Path { head, path })
        }),
    ]
}

fn expr() -> impl Strategy<Value = Expr> {
    leaf().prop_recursive(5, 48, 4, |inner| {
        prop_oneof![
            (inner.clone(), prop::sample::select(BinOp::all().to_vec()), inner.clone()).prop_map(|(l, op, r)| {
                e(ExprKind::Binary { op, lhs: Box::new(group(l)), rhs: Box::new(group(r)) })
            }),
            inner.clone().prop_map(|x| e(ExprKind::Unary { op: UnOp::Not, expr: Box::new(group(x)) })),
            inner.clone().prop_map(|x| e(ExprKind::Unary { op: UnOp::Neg, expr: Box::new(group(x)) })),
            prop::collection::vec(inner.clone(), 0..4).prop_map(|v| e(ExprKind::List(v))),
            prop::collection::vec(("[a-z ]{1,6}", inner.clone()), 0..3).prop_map(|kv| {
                let mut seen = std::collections::HashSet::new();
                let fields = kv
                    .into_iter()
                    .filter(|(k, _)| seen.insert(k.clone()))
                    .map(|(k, v)| (e(ExprKind::String(k)), v))
                    .collect();
                e(ExprKind::Record(fields))
            }),
            (prop::collection::vec(ident(), 0..3), inner.clone()).prop_map(|(params, body)| {
                let block =
                    Block { stmts: vec![Stmt::Pipeline(Pipeline { elements: vec![body], span: sp() })], span: sp() };
                e(ExprKind::Closure { params, body: Rc::new(block) })
            }),
            (inner.clone(), inner.clone(), any::<bool>()).prop_map(|(c, t, has_else)| {
                let blk = |x: Expr| {
                    Rc::new(Block {
                        stmts: vec![Stmt::Pipeline(Pipeline { elements: vec![x], span: sp() })],
                        span: sp(),
                    })
                };
                let else_ = has_else.then(|| Box::new(e(ExprKind::Block(blk(e(ExprKind::Int(0)))))));
                e(ExprKind::If { cond: Box::new(group(c)), then: blk(t), else_ })
            }),
        ]
    })
}

/// Debug output with all spans (`@a..b`) removed: structural equality.
fn shape<T: std::fmt::Debug>(x: &T) -> String {
    let d = format!("{x:?}");
    let mut out = String::with_capacity(d.len());
    let b: Vec<char> = d.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '@' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
            i += 1;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.') {
                i += 1;
            }
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn parse_one(src: &str) -> Expr {
    let ctx = Ctx::default();
    let (b, errs) = parse(src, &ctx);
    assert!(errs.is_empty(), "printed source failed to parse: {src:?}\n{errs:?}");
    assert_eq!(b.stmts.len(), 1, "{src:?}");
    match &b.stmts[0] {
        Stmt::Pipeline(p) if p.elements.len() == 1 => p.elements[0].clone(),
        s => panic!("unexpected statement for {src:?}: {s:?}"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// parse(print(ast)) == ast, modulo spans.
    #[test]
    fn print_parse_round_trip(ast in expr()) {
        let src = print_expr(&ast);
        let back = parse_one(&src);
        prop_assert_eq!(shape(&back), shape(&ast), "source: {}", src);
    }

    /// Formatting is idempotent: print(parse(s)) is a fixpoint.
    #[test]
    fn formatting_is_idempotent(ast in expr()) {
        let s1 = print_expr(&ast);
        let s2 = print_expr(&parse_one(&s1));
        prop_assert_eq!(&s1, &s2);
    }
}

const FRAGMENTS: &[&str] = &[
    "ls",
    "|",
    "where",
    "size",
    ">",
    "10kb",
    "{",
    "}",
    "(",
    ")",
    "[",
    "]",
    "$x",
    "$x.a.0",
    "let",
    "mut",
    "=",
    "def",
    "f",
    "[a: int]",
    "if",
    "else",
    "for",
    "in",
    "1..",
    "..<",
    "\"str",
    "\"ok\"",
    "'raw'",
    "$\"i(",
    "--flag",
    "-f",
    ";",
    "\n",
    ",",
    ":",
    "each",
    "{|x|",
    "not",
    "and",
    "or",
    "+",
    "-",
    "*",
    "**",
    "//",
    "mod",
    "^echo",
    "1.5",
    "-3",
    "null",
    "true",
    "str upcase",
    "get",
    "a.b",
    "#comment",
    "...rest",
    "?",
    "0x",
    "1_000",
    "while",
    "$in",
    "{}",
    "é",
    "🙂",
    "\\",
    "=~",
    "++",
];

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    /// The parser and checker never panic on token soup.
    #[test]
    fn fuzz_token_soup(parts in prop::collection::vec(prop::sample::select(FRAGMENTS.to_vec()), 0..40), seps in prop::collection::vec(any::<bool>(), 40)) {
        let mut src = String::new();
        for (i, p) in parts.iter().enumerate() {
            src.push_str(p);
            if seps[i] { src.push(' '); }
        }
        let mut eng = Engine::new();
        let _ = eng.check(&src);
    }

    /// ... nor on arbitrary unicode input.
    #[test]
    fn fuzz_arbitrary_strings(src in any::<String>()) {
        let mut eng = Engine::new();
        let _ = eng.check(&src);
    }
}
