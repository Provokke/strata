//! Golden end-to-end tests: every case runs `strata -c <cmd>` in
//! tests/fixtures with a fixed terminal width and compares stdout, stderr and
//! exit status with tests/golden/cases.txt.
//!
//! Regenerate with `BLESS=1 cargo test --test golden` (reads commands.txt);
//! review the diff before committing.

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn run(cmd: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_strata"))
        .args(["-c", cmd])
        .current_dir(root().join("fixtures"))
        .env("STRATA_WIDTH", "80")
        .output()
        .expect("spawn strata");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    if !s.is_empty() && !s.ends_with('\n') {
        s.push_str("\n--- (no trailing newline)\n");
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.is_empty() {
        s.push_str("--- stderr\n");
        s.push_str(&err);
    }
    if !out.status.success() {
        s.push_str(&format!("--- exit {}\n", out.status.code().unwrap_or(-1)));
    }
    s
}

fn parse_cases(text: &str) -> Vec<(String, String)> {
    let mut cases = Vec::new();
    let mut cur: Option<(String, String)> = None;
    for line in text.split_inclusive('\n') {
        if let Some(cmd) = line.strip_prefix("### ") {
            if let Some(c) = cur.take() {
                cases.push(c);
            }
            cur = Some((cmd.trim_end_matches('\n').to_string(), String::new()));
        } else if let Some((_, exp)) = cur.as_mut() {
            exp.push_str(line);
        }
    }
    cases.extend(cur);
    cases
}

#[test]
fn golden_cases() {
    let cases_path = root().join("golden/cases.txt");
    if std::env::var("BLESS").is_ok() {
        let cmds = std::fs::read_to_string(root().join("golden/commands.txt")).unwrap();
        let mut out = String::new();
        for c in cmds.lines().filter(|l| !l.trim().is_empty()) {
            out.push_str(&format!("### {c}\n{}", run(c)));
        }
        std::fs::write(&cases_path, out).unwrap();
        return;
    }
    let text = std::fs::read_to_string(&cases_path).expect("cases.txt (run with BLESS=1 to create)");
    let cases = parse_cases(&text);
    assert!(cases.len() >= 100, "expected at least 100 golden cases, found {}", cases.len());
    let mut failures = Vec::new();
    for (cmd, expected) in &cases {
        let actual = run(cmd);
        if &actual != expected {
            failures.push(format!("=== {cmd}\n--- expected\n{expected}--- actual\n{actual}"));
        }
    }
    assert!(failures.is_empty(), "{} of {} golden cases failed:\n{}", failures.len(), cases.len(), failures.join("\n"));
    eprintln!("{} golden cases passed", cases.len());
}
