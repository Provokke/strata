//! Evaluator tests through the public engine API.

use strata::engine::Engine;
use strata::value::{Val, Value};

fn run(src: &str) -> Value {
    let mut e = Engine::new();
    e.run_value(src).unwrap_or_else(|errs| panic!("{src:?} failed: {errs:?}"))
}

fn int(src: &str) -> i64 {
    run(src).as_int().unwrap()
}

fn s(src: &str) -> String {
    run(src).as_str().unwrap().to_string()
}

fn rt_err(src: &str) -> String {
    let mut e = Engine::new();
    match e.run_value(src) {
        Ok(v) => panic!("expected runtime error for {src:?}, got {v:?}"),
        Err(errs) => errs[0].msg.clone(),
    }
}

#[test]
fn arithmetic_and_types() {
    assert_eq!(int("1 + 2 * 3 - 4"), 3);
    assert_eq!(int("2 ** 8"), 256);
    assert_eq!(int("-7 // 2"), -4);
    assert_eq!(int("10 / 5"), 2);
    assert!(matches!(run("10 / 4").v, Val::Float(f) if f == 2.5));
    assert!(matches!(run("1.5 + 1").v, Val::Float(f) if f == 2.5));
    assert!(matches!(run("2kb * 2").v, Val::Filesize(4000)));
    assert!(matches!(run("1hr - 30min").v, Val::Duration(d) if d == 1_800_000_000_000));
    assert_eq!(s("\"a\" ++ \"b\""), "ab");
    assert!(rt_err("1 / 0").contains("division by zero"));
    assert!(rt_err("9223372036854775807 * 2").contains("overflow"));
}

#[test]
fn variables_scopes_and_mutation() {
    assert_eq!(int("let x = 1; let y = ($x + 1); $y"), 2);
    assert_eq!(int("mut t = 0; for i in 1..10 { $t += $i }; $t"), 55);
    // shadowing in an inner block does not leak
    assert_eq!(int("let x = 1; if true { let x = 2 }; $x"), 1);
    // closures capture by value at creation
    assert_eq!(int("mut a = 1; let f = {|| $a }; $a = 5; [0] | each $f | first"), 1);
}

#[test]
fn closures_capture_environment() {
    assert_eq!(int("let k = 10; [1 2 3] | each {|x| $x * $k} | math sum"), 60);
    assert_eq!(int("[1 2 3] | each { $in + 1 } | math sum"), 9);
    assert_eq!(int("[1 2 3] | reduce {|x, acc| $acc * 10 + $x}"), 123);
}

#[test]
fn custom_commands() {
    assert_eq!(int("def sq [x: int] { $x * $x }; sq 7"), 49);
    assert_eq!(int("def f [a: int, b: int = 10] { $a + $b }; f 1"), 11);
    assert_eq!(int("def f [...xs: int] { $xs | length }; f 1 2 3"), 3);
    assert_eq!(s("def f [--name(-n): string] { $name }; f -n bob"), "bob");
    assert_eq!(int("def fib [n: int] { if $n < 2 { $n } else { (fib ($n - 1)) + (fib ($n - 2)) } }; fib 15"), 610);
    assert_eq!(int("def total [] { $in | math sum }; [1 2 3] | total"), 6);
}

#[test]
fn cell_paths_and_structured_ops() {
    assert_eq!(int("let r = {a: [{b: 1}, {b: 2}]}; $r.a.1.b"), 2);
    assert_eq!(int("[{a: 1}, {a: 2}] | get a | math sum"), 3);
    assert!(matches!(run("let r = {a: 1}; $r.b?").v, Val::Nothing));
    assert_eq!(int("{a: {b: 1}} | update a.b 5 | get a.b"), 5);
    assert_eq!(int("{a: 1} | insert b.c 3 | get b.c"), 3);
    assert!(rt_err("{a: 1} | insert a 2").contains("already exists"));
    assert!(rt_err("{a: 1} | update z 2").contains("not found"));
    assert_eq!(s("[{n: b}, {n: a}] | sort-by n | get 0.n"), "a");
}

#[test]
fn format_round_trips() {
    assert_eq!(int("{a: [1 2 3]} | to json | from json | get a | math sum"), 6);
    assert_eq!(int("[{x: 1}, {x: 2}] | to csv | from csv | get x | math sum"), 3);
    assert_eq!(int("{t: {v: 9}} | to toml | from toml | get t.v"), 9);
}

#[test]
fn file_io_save_open() {
    let dir = std::env::temp_dir().join(format!("strata-eval-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("out.json");
    let _ = std::fs::remove_file(&p);
    let mut e = Engine::new();
    e.run_value(&format!("[{{a: 1}}, {{a: 2}}] | save {}", p.display())).unwrap();
    assert_eq!(e.run_value(&format!("open {} | get a | math sum", p.display())).unwrap().as_int().unwrap(), 3);
    // refuses to overwrite without --force
    assert!(e.run_value(&format!("[] | save {}", p.display())).is_err());
    e.run_value(&format!("[] | save -f {}", p.display())).unwrap();
    assert_eq!(e.run_value(&format!("open {} | length", p.display())).unwrap().as_int().unwrap(), 0);
}

#[test]
fn cd_pwd_and_relative_paths() {
    let dir = std::env::temp_dir().join(format!("strata-cd-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/f.txt"), "hi").unwrap();
    let mut e = Engine::new();
    e.run_value(&format!("cd {}", dir.display())).unwrap();
    e.run_value("cd sub").unwrap();
    assert!(e.run_value("pwd").unwrap().as_str().unwrap().ends_with("sub"));
    assert_eq!(e.run_value("open f.txt").unwrap().as_str().unwrap(), "hi");
    assert_eq!(e.run_value("ls | get name").unwrap().to_text(), "f.txt");
    assert!(e.run_value("cd does-not-exist").is_err());
}

#[test]
fn runtime_errors_have_spans() {
    let mut e = Engine::new();
    let errs = e.run_value("[1 2] | get 5").unwrap_err();
    assert_eq!(errs[0].span, strata::span::Span::new(12, 13));
    let rendered = errs[0].render("[1 2] | get 5", "t");
    assert!(rendered.contains("1 | [1 2] | get 5\n  |             ^ row 5 not found"), "{rendered}");
}
