//! Laziness: pipelines are iterator chains, so taking a prefix of a huge or
//! infinite source must return immediately.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn run_with_timeout(cmd: &str, limit: Duration) -> (String, Duration) {
    let start = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_strata"))
        .args(["-c", cmd])
        .env("STRATA_WIDTH", "80")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    loop {
        if let Some(_st) = child.try_wait().unwrap() {
            let out = child.wait_with_output().unwrap();
            return (String::from_utf8_lossy(&out.stdout).into_owned(), start.elapsed());
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            panic!("`{cmd}` did not finish within {limit:?}: pipeline is not lazy");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn billion_element_range_first_three_is_instant() {
    let (out, took) = run_with_timeout("range 1..1_000_000_000 | first 3 | to json --raw", Duration::from_secs(5));
    assert_eq!(out.trim(), "[1,2,3]");
    assert!(took < Duration::from_secs(2), "took {took:?}");
}

#[test]
fn infinite_range_through_each_and_where() {
    let (out, _) = run_with_timeout(
        "1.. | each {|x| $x * 3} | where $it mod 2 == 0 | first 4 | to json --raw",
        Duration::from_secs(5),
    );
    assert_eq!(out.trim(), "[6,12,18,24]");
}

#[test]
fn skip_take_enumerate_stay_lazy() {
    let (out, _) =
        run_with_timeout("0.. | skip 1_000 | take 2 | enumerate | get item | to json --raw", Duration::from_secs(5));
    assert_eq!(out.trim(), "[1000,1001]");
}

#[cfg(unix)]
#[test]
fn infinite_external_stream_is_read_lazily() {
    // `yes` never terminates on its own; `lines | first` must stop reading.
    let (out, _) = run_with_timeout("^yes strata | lines | first 2 | to json --raw", Duration::from_secs(5));
    assert_eq!(out.trim(), r#"["strata","strata"]"#);
}

#[test]
fn library_level_stream_is_pull_based() {
    use strata::engine::Engine;
    use strata::eval::PipelineData;
    let mut e = Engine::new();
    let out = e.run("1.. | each {|x| $x * 2}").unwrap();
    let PipelineData::Stream(mut it, _) = out else { panic!("expected a lazy stream") };
    let firsts: Vec<i64> = (0..3).map(|_| it.next().unwrap().unwrap().as_int().unwrap()).collect();
    assert_eq!(firsts, vec![2, 4, 6]);
}
