//! Type checker accept/reject tests. Rejections must happen before execution.

use strata::engine::Engine;
use strata::span::{ErrorKind, Span};

fn accept(src: &str) {
    let mut e = Engine::new();
    let errs = e.check(src);
    assert!(errs.is_empty(), "expected {src:?} to type-check, got {errs:?}");
}

fn reject(src: &str, msg_part: &str) -> Span {
    let mut e = Engine::new();
    let errs = e.check(src);
    assert!(!errs.is_empty(), "expected {src:?} to be rejected");
    let hit = errs.iter().find(|x| x.msg.contains(msg_part));
    let hit = hit.unwrap_or_else(|| panic!("no error containing {msg_part:?} in {errs:?}"));
    assert_eq!(hit.kind, ErrorKind::Type);
    hit.span
}

#[test]
fn accepts_well_typed_programs() {
    for src in [
        "1 + 2",
        "1 + 2.5",
        "\"a\" + \"b\"",
        "1kb + 2kb",
        "1sec * 3",
        "[1 2 3] | math sum",
        "1..10 | math sum",
        "ls | where size > 1kb | get name",
        "ls | length",
        "[a b] | str upcase",
        "\"x\" | str upcase | str length",
        "let x: int = 5; $x * 2",
        "let x = [1 2]; $x | length",
        "def f [x: int] { $x + 1 }; f 2",
        "def f [x: int]: nothing -> int { $x + 1 }",
        "def g [--n: int] { $n }; g --n 3",
        "mut a = 1; $a = 2; $a += 3",
        "if true { 1 } else { 2 }",
        "for x in [1 2] { $x }",
        "for x in 1..3 { $x }",
        "[1 2] | each {|x| $x * 2} | math max",
        "{a: 1} | get a",
        "$env.HOME",
        "open x.json | where a > 1",
        "^echo hi | lines",
        "^echo 5 | into int",
        "[{a: 1}] | update a 2",
        "\"a\" in [\"a\"]",
        "\"a\" =~ \"b\"",
        "(1 + 2) * 3",
        "[1 2] | reduce -f 0 {|x, acc| $acc + $x}",
    ] {
        accept(src);
    }
}

#[test]
fn rejects_operator_type_errors() {
    assert_eq!(reject("1 + \"a\"", "operator '+' is not supported between int and string"), Span::new(0, 7));
    assert_eq!(reject("let a = 3; $a - \"x\"", "between int and string"), Span::new(11, 19));
    reject("\"a\" < 1", "operator '<'");
    reject("1kb + 1", "between filesize and int");
    reject("true and 1", "'and' expects bool");
    assert_eq!(reject("not 5", "'not' expects bool"), Span::new(4, 5));
    reject("-\"s\"", "cannot negate string");
    reject("1 in 5", "operator 'in'");
    // `$in` carries the static type of the pipeline input
    reject("\"x\" | $in - 1", "between string and int");
}

#[test]
fn rejects_pipeline_input_mismatches() {
    assert_eq!(reject("\"abc\" | math sum", "does not accept string input"), Span::new(8, 16));
    reject("5 | str upcase", "does not accept int");
    reject("[1 2] | str upcase", "does not accept list<int>");
    reject("{a: 1} | length", "does not accept record");
    reject("first 3", "does not accept nothing");
    reject("{a: 1} | to toml | to toml", "does not accept string");
    reject("ls | length | first", "does not accept int");
}

#[test]
fn rejects_argument_errors() {
    assert_eq!(reject("[1] | first \"x\"", "expected int, found string"), Span::new(12, 15));
    reject("\"a\" | str replace x", "missing required argument 'replace'");
    reject("def f [a: int] { $a }; f \"s\"", "expected int, found string");
    reject("def f [a: int] { $a }; f", "missing required argument 'a'");
    reject("def f [] { 1 }; f 1", "extra positional argument");
    reject("def g [--n: int] { $n }; g --n \"x\"", "type mismatch for flag --n");
    reject("[1 2] | each 5", "expected closure, found int");
}

#[test]
fn rejects_variable_and_structure_errors() {
    assert_eq!(reject("$nope", "variable '$nope' not found"), Span::new(0, 5));
    reject("let x: int = \"s\"", "declared as int but the value is string");
    reject("let x: string = 1 + 2", "declared as string");
    reject("mut a = 1; $a = \"s\"", "cannot assign string");
    reject("$q = 1", "variable '$q' not found");
    reject("let n = 1; $n.a", "cannot access 'a' on a value of type int");
    reject("if 1 { 2 }", "if condition must be bool");
    reject("while 1 { }", "while condition must be bool");
    reject("for x in 5 { }", "cannot iterate over int");
    reject("let s = \"a\"; 1..$s", "range bounds must be int");
    reject("def f [x: int]: nothing -> string { $x }", "declares output string");
    reject("[1 2] | where foo > 1", "cannot access a column of int");
    // closures see their parameters, scoping is respected
    reject("[1] | each {|x| $y}", "variable '$y' not found");
    reject("for x in [1] { }; $x", "variable '$x' not found");
}

#[test]
fn type_errors_prevent_execution() {
    let dir = std::env::temp_dir().join(format!("strata-chk-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("should_not_exist.txt");
    let mut e = Engine::new();
    let src = format!("\"x\" | save {}; 1 + \"oops\"", f.display());
    assert!(e.run(&src).is_err());
    assert!(!f.exists(), "side effect ran despite type error");
}

#[test]
fn repl_remembers_variable_types() {
    let mut e = Engine::new();
    e.run_value("let s = \"text\"").unwrap();
    let errs = e.check("$s | math sum");
    assert_eq!(errs.len(), 1);
    assert!(errs[0].msg.contains("does not accept string"));
}
