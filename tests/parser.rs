//! Parser tests: structure, precedence, and error recovery with exact spans.

use strata::ast::*;
use strata::combinator::Ctx;
use strata::engine::Engine;
use strata::parser::parse;
use strata::span::Span;

fn ctx() -> Ctx {
    // a context pre-loaded with builtin signatures
    let e = Engine::new();
    let c = Ctx::default();
    *c.sigs.borrow_mut() = e.ctx.sigs.borrow().clone();
    c
}

fn ok(src: &str) -> Block {
    let c = ctx();
    let (b, errs) = parse(src, &c);
    assert!(errs.is_empty(), "unexpected errors for {src:?}: {errs:?}");
    b
}

fn errs(src: &str) -> Vec<(String, Span)> {
    let c = ctx();
    let (_, e) = parse(src, &c);
    e.into_iter().map(|e| (e.msg, e.span)).collect()
}

fn first_expr(b: &Block) -> &Expr {
    match &b.stmts[0] {
        Stmt::Pipeline(p) => &p.elements[0],
        s => panic!("not a pipeline: {s:?}"),
    }
}

fn printed(src: &str) -> String {
    print_block(&ok(src))
}

#[test]
fn precedence_and_associativity() {
    assert_eq!(printed("1 + 2 * 3"), "1 + 2 * 3");
    assert_eq!(printed("1 - 2 - 3"), "1 - 2 - 3");
    assert_eq!(printed("2 ** 3 ** 2"), "2 ** 3 ** 2");
    assert_eq!(printed("1 < 2 and 3 > 4 or true"), "1 < 2 and 3 > 4 or true");
    assert_eq!(printed("not true and false"), "not true and false");
    assert_eq!(printed("-$x * 2"), "- $x * 2");
    assert_eq!(printed("1 + 2 mod 3 // 4"), "1 + 2 mod 3 // 4");
    // explicit grouping survives as a sub-expression
    assert_eq!(printed("(1 + 2) * 3"), "(1 + 2) * 3");
    assert_eq!(printed("2 * (3 - 1)"), "2 * (3 - 1)");
    assert_eq!(printed("\"a\" ++ \"b\" == \"ab\""), "\"a\" ++ \"b\" == \"ab\"");
}

#[test]
fn pipelines_and_calls() {
    let b = ok("ls | where size > 10kb | sort-by name --reverse | first 3");
    let Stmt::Pipeline(p) = &b.stmts[0] else { panic!() };
    assert_eq!(p.elements.len(), 4);
    let ExprKind::Call(c) = &p.elements[2].kind else { panic!() };
    assert_eq!(c.name, "sort-by");
    assert_eq!(c.name_span, Span::new(25, 32));
    assert!(matches!(&c.args[1], Arg::Named { name, .. } if name == "reverse"));
    // multi-word command names
    let b = ok("\"x\" | str upcase");
    let Stmt::Pipeline(p) = &b.stmts[0] else { panic!() };
    let ExprKind::Call(c) = &p.elements[1].kind else { panic!() };
    assert_eq!(c.name, "str upcase");
    assert_eq!(c.name_span, Span::new(6, 16));
    // short flags resolve to long names
    assert_eq!(printed("[3 1] | sort -r"), "[3, 1] | sort --reverse");
}

#[test]
fn row_condition_becomes_closure_with_columns() {
    assert_eq!(printed("ls | where size > 1kb"), "ls | where {|| size > 1000b}");
    // bare rhs of a comparison against a column is a string
    assert_eq!(printed("ls | where type == file"), "ls | where {|| type == \"file\"}");
    // a literal lhs leaves the rhs as a column
    assert_eq!(printed("ls | where \"x\" in tags"), "ls | where {|| \"x\" in tags}");
    assert_eq!(printed("ls | where {|r| $r.size > 0}"), "ls | where {|r| $r.size > 0}");
}

#[test]
fn let_mut_def_for_if() {
    assert_eq!(printed("let x: int = 1 + 2"), "let x: int = 1 + 2");
    assert_eq!(printed("mut y = [1 2]"), "mut y = [1, 2]");
    assert_eq!(printed("$y += 1"), "$y += 1");
    assert_eq!(
        printed("def f [a: int, b?: string, ...rest: int, --flag(-f), --n: int] { $a }"),
        "def \"f\" [a: int, b?: string, ...rest: int, --flag(-f), --n: int] { $a }"
    );
    assert_eq!(printed("for x in 1..3 { $x }"), "for x in 1..3 { $x }");
    assert_eq!(printed("if $a { 1 } else if $b { 2 } else { 3 }"), "if $a { 1 } else if $b { 2 } else { 3 }");
    let b = ok("def f [x: int = 5] { $x }");
    let Stmt::Def(d) = &b.stmts[0] else { panic!() };
    assert!(d.sig.positional[0].optional);
    assert_eq!(d.sig.positional[0].default.as_ref().unwrap().as_int().unwrap(), 5);
}

#[test]
fn literals_cellpaths_ranges_interp() {
    assert_eq!(printed("$x.a.0.b?"), "$x.a.0.b?");
    assert_eq!(printed("(ls).name"), "(ls).name");
    assert_eq!(printed("1..10"), "1..10");
    assert_eq!(printed("0..<$n"), "0..<$n");
    assert_eq!(printed("5.."), "5..");
    assert_eq!(printed("{a: 1, \"b c\": [x y]}"), "{\"a\": 1, \"b c\": [\"x\", \"y\"]}");
    assert_eq!(printed("$\"a(1 + 2)b\""), "$\"a(1 + 2)b\"");
    assert_eq!(printed("{|a, b| $a + $b}"), "{|a, b| $a + $b}");
    assert_eq!(printed("[1, 2\n 3]"), "[1, 2, 3]");
    // spans of an interpolated sub-expression point into the original source
    let b = ok("$\"v=($x)\"");
    let ExprKind::Interp(parts) = &first_expr(&b).kind else { panic!() };
    let ExprKind::Subexpr(inner) = &parts[1].kind else { panic!() };
    assert_eq!(first_expr(inner).span, Span::new(5, 7));
}

#[test]
fn statements_separated_and_multiline_pipes() {
    let b = ok("let a = 1; let b = 2\n$a + $b");
    assert_eq!(b.stmts.len(), 3);
    let b = ok("ls\n  | where size > 1\n  | length");
    let Stmt::Pipeline(p) = &b.stmts[0] else { panic!() };
    assert_eq!(p.elements.len(), 3);
}

#[test]
fn externals() {
    assert_eq!(printed("^echo hi --flag"), "^\"echo\" \"hi\" \"--flag\"");
    assert_eq!(printed("unknowncmd a b"), "^\"unknowncmd\" \"a\" \"b\"");
}

#[test]
fn error_spans_are_exact() {
    assert_eq!(errs("echo (1 + )"), vec![("expected expression, found ')'".to_string(), Span::new(10, 11))]);
    assert_eq!(
        errs("[1 2"),
        vec![("unclosed delimiter: expected ']', found end of input".to_string(), Span::new(0, 1))]
    );
    assert_eq!(errs("let = 5"), vec![("expected name or variable name, found '='".to_string(), Span::new(4, 5))]);
    assert_eq!(errs("ls --bogus"), vec![("unknown flag --bogus for command 'ls'".to_string(), Span::new(3, 10))]);
    assert_eq!(errs("echo \"abc"), vec![("unterminated string".to_string(), Span::new(5, 9))]);
    assert_eq!(errs("1 + foo"), vec![("unexpected bareword 'foo' in expression".to_string(), Span::new(4, 7))]);
    assert_eq!(errs("let x: nope = 1"), vec![("unknown type 'nope'".to_string(), Span::new(7, 11))]);
    assert_eq!(errs("sort -r --x"), vec![("unknown flag --x for command 'sort'".to_string(), Span::new(8, 11))]);
    assert_eq!(errs("echo )"), vec![("unexpected ')'".to_string(), Span::new(5, 6))]);
}

#[test]
fn recovery_reports_multiple_errors_and_keeps_good_statements() {
    let c = ctx();
    let (b, e) = parse("let a = 1\nlet = 2\necho (1 +)\nlet c = 3", &c);
    let got: Vec<(String, Span)> = e.into_iter().map(|e| (e.msg, e.span)).collect();
    assert_eq!(
        got,
        vec![
            ("expected name or variable name, found '='".to_string(), Span::new(14, 15)),
            ("expected expression, found ')'".to_string(), Span::new(27, 28)),
        ]
    );
    assert_eq!(b.stmts.len(), 4);
    assert!(matches!(b.stmts[0], Stmt::Let { .. }));
    assert_eq!(b.stmts[1], Stmt::Error(Span::new(10, 17)));
    // the error inside ( ) is recovered at the inner statement, so `echo` survives
    assert_eq!(print_stmt(&b.stmts[2]), "echo (<error>)");
    assert!(matches!(&b.stmts[3], Stmt::Let { name, .. } if name == "c"));
}

#[test]
fn recovery_inside_blocks() {
    let c = ctx();
    let (b, e) = parse("def f [] { let = 1; 2 }; f", &c);
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].span, Span::new(15, 16));
    let Stmt::Def(d) = &b.stmts[0] else { panic!("{:?}", b.stmts) };
    assert_eq!(d.body.stmts.len(), 2);
    assert!(matches!(d.body.stmts[0], Stmt::Error(_)));
}

#[test]
fn deep_nesting_is_an_error_not_a_crash() {
    let src = "(".repeat(5000) + &")".repeat(5000);
    let e = errs(&src);
    assert!(!e.is_empty());
    assert!(e[0].0.contains("nesting too deep"), "{e:?}");
}
