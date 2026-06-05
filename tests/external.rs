//! External command interop: byte streams, auto-conversion both ways, exit codes.
#![cfg(unix)]

use strata::engine::Engine;

fn val(src: &str) -> strata::value::Value {
    Engine::new().run_value(src).unwrap_or_else(|e| panic!("{src}: {e:?}"))
}

#[test]
fn external_output_becomes_string_for_internal_commands() {
    assert_eq!(val("^echo hello | str upcase").as_str().unwrap(), "HELLO");
    assert_eq!(val("^printf 'a\\nb\\nc\\n' | lines | length").as_int().unwrap(), 3);
    assert_eq!(val("^printf '{\"k\": [1,2]}' | from json | get k.1").as_int().unwrap(), 2);
    assert_eq!(val("let x = (^echo captured); $x").as_str().unwrap(), "captured");
}

#[test]
fn internal_values_feed_external_stdin() {
    assert_eq!(val("\"zeta\\nalpha\" | ^sort | lines | first").as_str().unwrap(), "alpha");
    assert_eq!(val("[3 1 2] | ^sort -n | lines | str join ','").as_str().unwrap(), "1,2,3");
    // tables are rendered as text
    let t = val("[{name: x}] | ^cat").as_str().unwrap().to_string();
    assert!(t.contains("│ name │"), "{t}");
}

#[test]
fn chained_externals() {
    assert_eq!(val("^printf 'b\\na\\n' | ^sort | ^head -n 1").as_str().unwrap(), "a");
}

#[test]
fn large_stdin_does_not_deadlock() {
    // 2 MB through `cat` must not deadlock (stdin is written on a thread).
    let v = val("1..200_000 | each {|x| 'abcdefghi' } | str join \"\\n\" | ^cat | str length");
    assert_eq!(v.as_int().unwrap(), 200_000 * 10 - 1);
}

#[test]
fn failures_are_reported() {
    let mut e = Engine::new();
    let errs = e.run_value("^sh -c 'exit 7'").unwrap_err();
    assert!(errs[0].msg.contains("exited with 7"), "{:?}", errs);
    let errs = e.run_value("^no-such-binary-strata-test").unwrap_err();
    assert!(errs[0].msg.contains("not found"));
}

#[test]
fn arguments_from_variables_and_globs() {
    assert_eq!(val("let w = \"from var\"; ^echo $w").as_str().unwrap(), "from var");
    let dir = std::env::temp_dir().join(format!("strata-glob-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["a.txt", "b.txt", "c.md"] {
        std::fs::write(dir.join(f), "").unwrap();
    }
    let mut e = Engine::new();
    e.run_value(&format!("cd {}", dir.display())).unwrap();
    assert_eq!(e.run_value("^echo *.txt").unwrap().as_str().unwrap(), "a.txt b.txt");
    assert_eq!(e.run_value("ls *.md | get name | first").unwrap().as_str().unwrap(), "c.md");
}
