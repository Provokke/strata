//! Pipeline benchmarks over 1M rows (run: `cargo bench`).
//! Plain harness: each case runs a few times and reports the best wall time.

use std::time::{Duration, Instant};
use strata::engine::Engine;

fn bench(name: &str, setup: &str, src: &str, iters: u32) {
    let mut best = Duration::MAX;
    let mut result = String::new();
    for _ in 0..iters {
        let mut e = Engine::new();
        if !setup.is_empty() {
            e.run_value(setup).unwrap();
        }
        let t = Instant::now();
        let v = e.run_value(src).unwrap_or_else(|err| panic!("{src}: {err:?}"));
        best = best.min(t.elapsed());
        result = v.to_display();
    }
    println!("{name:<48} {:>9.1} ms   (result: {result})", best.as_secs_f64() * 1e3);
}

fn main() {
    let dir = std::env::temp_dir().join("strata-bench");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("rows.csv");
    if !csv.exists() {
        let mut s = String::from("id,group,value\n");
        for i in 0..1_000_000u64 {
            s.push_str(&format!("{i},g{},{}\n", i % 10, (i * 7919) % 1000));
        }
        std::fs::write(&csv, s).unwrap();
    }
    let n = 3;
    println!("strata pipeline benchmarks (best of {n})");
    bench("range 1M | math sum", "", "0..<1_000_000 | math sum", n);
    bench(
        "range 1M | each {x*2} | where | length",
        "",
        "0..<1_000_000 | each {|x| $x * 2} | where $it mod 3 == 0 | length",
        n,
    );
    bench(
        "1M records: build | where col | length",
        "",
        "0..<1_000_000 | each {|x| {a: $x, b: ($x mod 7)}} | where b == 3 | length",
        n,
    );
    bench("range 1G | first 3 (laziness)", "", "range 1..1_000_000_000 | first 3 | length", n);
    bench(
        "open 1M-row csv | where value > 500 | length",
        "",
        &format!("open {} | where value > 500 | length", csv.display()),
        n,
    );
    bench(
        "1M-row table: get value | math avg",
        &format!("let t = (open {})", csv.display()),
        "$t | get value | math avg",
        n,
    );
    bench(
        "1M-row table: sort-by value | first",
        &format!("let t = (open {})", csv.display()),
        "$t | sort-by value | first | get id",
        n,
    );
    bench(
        "1M-row table: group-by group | columns",
        &format!("let t = (open {})", csv.display()),
        "$t | group-by group | columns | length",
        n,
    );
}
