use std::io::{IsTerminal, Read, Write};
use strata::engine::Engine;

fn usage() -> ! {
    eprintln!("usage: strata [-c <commands>] [script.st] [--check]");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut engine = Engine::new();
    let mut stdout = std::io::stdout().lock();
    let check_only = args.iter().any(|a| a == "--check");
    let args: Vec<&String> = args.iter().filter(|a| *a != "--check").collect();
    let (src, file) = match args.as_slice() {
        [] => {
            if std::io::stdin().is_terminal() {
                repl(&mut engine);
                return;
            }
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).unwrap_or_default();
            (s, "<stdin>".to_string())
        }
        [c, cmd] if c.as_str() == "-c" => ((*cmd).clone(), "<commandline>".to_string()),
        [path] if !path.starts_with('-') => match std::fs::read_to_string(path) {
            Ok(s) => (s, (*path).clone()),
            Err(e) => {
                eprintln!("error: cannot read {path}: {e}");
                std::process::exit(1);
            }
        },
        _ => usage(),
    };
    if check_only {
        let errs = engine.check(&src);
        for e in &errs {
            eprint!("{}", e.render(&src, &file));
        }
        std::process::exit(if errs.is_empty() { 0 } else { 1 });
    }
    if let Err(e) = engine.run_and_print(&src, &file, &mut stdout) {
        let _ = stdout.flush();
        eprint!("{e}");
        std::process::exit(1);
    }
}

fn repl(engine: &mut Engine) {
    let mut rl = match rustyline::DefaultEditor::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot start line editor: {e}");
            return;
        }
    };
    #[cfg(windows)]
    let home = std::env::var("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var("HOME");
    let hist = home.map(|h| std::path::PathBuf::from(h).join(".strata_history")).ok();
    if let Some(h) = &hist {
        let _ = rl.load_history(h);
    }
    println!("strata {} — type `help` for commands, Ctrl-D to exit", env!("CARGO_PKG_VERSION"));
    loop {
        let cwd = engine.eval.g.cwd.borrow().display().to_string();
        match rl.readline(&format!("{cwd}> ")) {
            Ok(line) => {
                if line.trim().is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(line.as_str());
                if line.trim() == "exit" {
                    break;
                }
                let mut out = std::io::stdout().lock();
                if let Err(e) = engine.run_and_print(&line, "<repl>", &mut out) {
                    eprint!("{e}");
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => continue,
            Err(_) => break,
        }
    }
    if let Some(h) = &hist {
        let _ = rl.save_history(h);
    }
}
