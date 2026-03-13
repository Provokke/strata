//! Ties parsing, checking, evaluation and output together.

use crate::checker;
use crate::combinator::Ctx;
use crate::eval::{Eval, Globals, PipelineData};
use crate::parser;
use crate::span::{ShellError, Span};
use crate::types::Type;
use crate::value::{Val, Value};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::rc::Rc;

pub struct Engine {
    pub ctx: Ctx,
    pub eval: Eval,
    /// Static types of variables defined by earlier inputs (REPL).
    pub var_types: HashMap<String, Type>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Engine {
    pub fn new() -> Engine {
        let builtins = crate::commands::builtins();
        let ctx = Ctx::default();
        for (n, b) in &builtins {
            ctx.sigs.borrow_mut().insert(n.clone(), b.sig.clone());
        }
        let env: BTreeMap<String, String> = std::env::vars().collect();
        let width = std::env::var("STRATA_WIDTH")
            .ok()
            .and_then(|w| w.parse().ok())
            .or_else(|| terminal_size::terminal_size().map(|(w, _)| w.0 as usize))
            .unwrap_or(100);
        let g = Globals {
            builtins,
            defs: RefCell::new(HashMap::new()),
            cwd: RefCell::new(std::env::current_dir().unwrap_or_else(|_| ".".into())),
            env: RefCell::new(env),
            width: Cell::new(width),
        };
        Engine { ctx, eval: Eval::new(Rc::new(g)), var_types: HashMap::new() }
    }

    /// Parse + type-check without running. Returns all errors found.
    pub fn check(&mut self, src: &str) -> Vec<ShellError> {
        let (block, mut errs) = parser::parse(src, &self.ctx);
        if errs.is_empty() {
            errs = checker::check(&block, &self.ctx.sigs.borrow(), &self.var_types);
        }
        errs
    }

    /// Run source text, returning the raw pipeline output.
    pub fn run(&mut self, src: &str) -> Result<PipelineData, Vec<ShellError>> {
        let (block, errs) = parser::parse(src, &self.ctx);
        if !errs.is_empty() {
            return Err(errs);
        }
        let errs = checker::check(&block, &self.ctx.sigs.borrow(), &self.var_types);
        if !errs.is_empty() {
            return Err(errs);
        }
        let out = self.eval.run_block(&block, PipelineData::Empty).map_err(|e| vec![e])?;
        // remember variable types for later REPL lines
        for s in &block.stmts {
            if let crate::ast::Stmt::Let { name, .. } = s
                && let Ok(v) = self.eval.lookup(name, Span::unknown())
            {
                let t = match &v.v {
                    Val::List(_) if v.type_name() == "table" => Type::Table,
                    _ => Type::of_value(&v),
                };
                self.var_types.insert(name.clone(), t);
            }
        }
        Ok(out)
    }

    /// Run and collect into a value (handy for tests).
    pub fn run_value(&mut self, src: &str) -> Result<Value, Vec<ShellError>> {
        let out = self.run(src)?;
        out.into_value(Span::unknown()).map_err(|e| vec![e])
    }

    /// Run and write the rendered output. Returns rendered errors on failure.
    pub fn run_and_print(&mut self, src: &str, file: &str, out: &mut dyn Write) -> Result<(), String> {
        let render = |errs: Vec<ShellError>| errs.iter().map(|e| e.render(src, file)).collect::<Vec<_>>().join("\n");
        let data = self.run(src).map_err(render)?;
        self.print(data, out).map_err(|e| render(vec![e]))
    }

    pub fn print(&self, data: PipelineData, out: &mut dyn Write) -> Result<(), ShellError> {
        let width = self.eval.g.width.get();
        let io = |e: std::io::Error| ShellError::rt(e.to_string(), Span::unknown());
        match data {
            PipelineData::Empty => Ok(()),
            PipelineData::Bytes(mut bs) => {
                std::io::copy(&mut bs.reader, out).map_err(io)?;
                bs.finish()
            }
            other => {
                let v = other.into_value(Span::unknown())?;
                let text = match &v.v {
                    Val::String(s) => format!("{s}\n"),
                    Val::Nothing => String::new(),
                    Val::Error(e) => return Err((**e).clone()),
                    _ => crate::table::render(&v, width),
                };
                out.write_all(text.as_bytes()).map_err(io)
            }
        }
    }
}
