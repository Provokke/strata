//! Evaluator: lazy streaming pipelines, closures, externals.

use crate::ast::*;
use crate::span::{ShellError, Span};
use crate::types::Signature;
use crate::value::*;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command as Proc, Stdio};
use std::rc::Rc;

pub type ValueIter = Box<dyn Iterator<Item = Result<Value, ShellError>>>;

/// Data flowing between pipeline elements.
pub enum PipelineData {
    Empty,
    Value(Value),
    /// A lazy stream of values.
    Stream(ValueIter, Span),
    /// Raw bytes from an external command's stdout.
    Bytes(ByteStream),
}

pub struct ByteStream {
    pub reader: Box<dyn Read>,
    pub child: Option<Child>,
    pub name: String,
    pub span: Span,
}

impl ByteStream {
    /// Wait for the child and turn a non-zero exit into an error.
    pub fn finish(&mut self) -> Result<(), ShellError> {
        if let Some(mut c) = self.child.take() {
            let st = c.wait().map_err(|e| ShellError::rt(e.to_string(), self.span))?;
            if !st.success() {
                return Err(ShellError::rt(
                    format!("external command '{}' exited with {}", self.name, st.code().unwrap_or(-1)),
                    self.span,
                ));
            }
        }
        Ok(())
    }
    pub fn collect_bytes(mut self) -> Result<Vec<u8>, ShellError> {
        let mut buf = Vec::new();
        self.reader.read_to_end(&mut buf).map_err(|e| ShellError::rt(e.to_string(), self.span))?;
        self.finish()?;
        Ok(buf)
    }
}

impl PipelineData {
    pub fn value(v: Value) -> PipelineData {
        PipelineData::Value(v)
    }

    pub fn stream(it: impl Iterator<Item = Result<Value, ShellError>> + 'static, span: Span) -> PipelineData {
        PipelineData::Stream(Box::new(it), span)
    }

    /// Collect into a single value (streams become lists).
    pub fn into_value(self, span: Span) -> Result<Value, ShellError> {
        match self {
            PipelineData::Empty => Ok(Value::nothing(span)),
            PipelineData::Value(v) => Ok(v),
            PipelineData::Stream(it, sp) => Ok(Value::list(it.collect::<Result<Vec<_>, _>>()?, sp)),
            PipelineData::Bytes(bs) => {
                let sp = bs.span;
                let bytes = bs.collect_bytes()?;
                match String::from_utf8(bytes) {
                    Ok(mut s) => {
                        if s.ends_with('\n') {
                            s.pop();
                            if s.ends_with('\r') {
                                s.pop();
                            }
                        }
                        Ok(Value::string(s, sp))
                    }
                    Err(e) => Ok(Value::new(Val::Binary(e.into_bytes()), sp)),
                }
            }
        }
    }

    /// Iterate items: lists/ranges/streams yield elements, scalars yield themselves.
    pub fn into_iter_values(self, span: Span) -> ValueIter {
        match self {
            PipelineData::Empty => Box::new(std::iter::empty()),
            PipelineData::Value(v) => value_iter(v),
            PipelineData::Stream(it, _) => it,
            PipelineData::Bytes(bs) => {
                let v = PipelineData::Bytes(bs).into_value(span);
                match v {
                    Ok(v) => value_iter(v),
                    Err(e) => Box::new(std::iter::once(Err(e))),
                }
            }
        }
    }

    pub fn span_or(&self, s: Span) -> Span {
        match self {
            PipelineData::Value(v) => v.span,
            PipelineData::Stream(_, sp) => *sp,
            PipelineData::Bytes(b) => b.span,
            PipelineData::Empty => s,
        }
    }
}

pub fn value_iter(v: Value) -> ValueIter {
    let span = v.span;
    match v.v {
        Val::List(l) => Box::new(l.into_iter().map(Ok)),
        Val::Range(r) => Box::new(r.iter().map(move |i| Ok(Value::int(i, span)))),
        Val::Nothing => Box::new(std::iter::empty()),
        _ => Box::new(std::iter::once(Ok(v))),
    }
}

/// Evaluated call arguments handed to builtins.
pub struct CallArgs {
    pub positional: Vec<Value>,
    pub named: HashMap<String, Option<Value>>,
    pub head: Span,
    pub span: Span,
}

impl CallArgs {
    pub fn req(&self, i: usize) -> Result<&Value, ShellError> {
        self.positional.get(i).ok_or_else(|| ShellError::rt(format!("missing required argument #{}", i + 1), self.head))
    }
    pub fn opt(&self, i: usize) -> Option<&Value> {
        self.positional.get(i)
    }
    pub fn has(&self, name: &str) -> bool {
        self.named.contains_key(name)
    }
    pub fn flag(&self, name: &str) -> Option<&Value> {
        self.named.get(name).and_then(|v| v.as_ref())
    }
    pub fn closure(&self, i: usize) -> Result<Closure, ShellError> {
        let v = self.req(i)?;
        match &v.v {
            Val::Closure(c) => Ok(c.clone()),
            _ => Err(ShellError::rt(format!("expected closure, found {}", v.type_name()), v.span)),
        }
    }
}

pub type BuiltinFn = fn(&mut Eval, &CallArgs, PipelineData) -> Result<PipelineData, ShellError>;

pub struct Builtin {
    pub sig: Signature,
    pub run: BuiltinFn,
}

/// Process-wide shell state shared by every evaluator (including those running
/// closures inside lazy streams).
pub struct Globals {
    pub builtins: HashMap<String, Builtin>,
    pub defs: RefCell<HashMap<String, Rc<Def>>>,
    pub cwd: RefCell<PathBuf>,
    pub env: RefCell<BTreeMap<String, String>>,
    pub width: Cell<usize>,
}

impl Globals {
    pub fn resolve(&self, p: &str) -> PathBuf {
        let path = PathBuf::from(p);
        if path.is_absolute() { path } else { self.cwd.borrow().join(path) }
    }
}

pub struct Eval {
    pub g: Rc<Globals>,
    captures: Rc<HashMap<String, Value>>,
    scopes: Vec<HashMap<String, Value>>,
}

/// A closure bundled with what it needs to run anywhere (e.g. inside a lazy iterator).
#[derive(Clone)]
pub struct ClosureRunner {
    pub g: Rc<Globals>,
    pub c: Closure,
}

impl ClosureRunner {
    pub fn call(&self, input: Value, extra: &[Value]) -> Result<Value, ShellError> {
        let mut ev = Eval { g: self.g.clone(), captures: self.c.captures.clone(), scopes: vec![HashMap::new()] };
        let scope = ev.scopes.last_mut().unwrap();
        scope.insert("in".into(), input.clone());
        scope.insert("it".into(), input.clone());
        let mut args = std::iter::once(input).chain(extra.iter().cloned());
        for p in &self.c.params {
            let v = args.next().unwrap_or_else(|| Value::nothing(Span::unknown()));
            scope.insert(p.clone(), v);
        }
        let span = self.c.body.span;
        let first = ev.scopes[0].get("in").cloned().unwrap();
        ev.run_block(&self.c.body, PipelineData::Value(first))?.into_value(span)
    }
}

impl Eval {
    pub fn new(g: Rc<Globals>) -> Eval {
        Eval { g, captures: Rc::new(HashMap::new()), scopes: vec![HashMap::new()] }
    }

    pub fn runner(&self, c: Closure) -> ClosureRunner {
        ClosureRunner { g: self.g.clone(), c }
    }

    pub fn lookup(&self, name: &str, span: Span) -> Result<Value, ShellError> {
        for s in self.scopes.iter().rev() {
            if let Some(v) = s.get(name) {
                return Ok(v.clone());
            }
        }
        if let Some(v) = self.captures.get(name) {
            return Ok(v.clone());
        }
        if name == "env" {
            let env = self.g.env.borrow();
            let r = env.iter().map(|(k, v)| (k.clone(), Value::string(v.clone(), span))).collect();
            return Ok(Value::record(r, span));
        }
        if name == "nothing" {
            return Ok(Value::nothing(span));
        }
        Err(ShellError::rt(format!("variable '${name}' not found"), span))
    }

    pub fn set_var(&mut self, name: &str, v: Value) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), v);
    }

    fn assign(&mut self, name: &str, v: Value, span: Span) -> Result<(), ShellError> {
        for s in self.scopes.iter_mut().rev() {
            if let Some(slot) = s.get_mut(name) {
                *slot = v;
                return Ok(());
            }
        }
        if self.captures.contains_key(name) {
            return Err(ShellError::rt(format!("cannot assign to captured variable '${name}'"), span));
        }
        Err(ShellError::rt(format!("variable '${name}' not found"), span))
    }

    fn capture(&self) -> Rc<HashMap<String, Value>> {
        let mut m: HashMap<String, Value> = (*self.captures).clone();
        for s in &self.scopes {
            for (k, v) in s {
                m.insert(k.clone(), v.clone());
            }
        }
        Rc::new(m)
    }

    pub fn run_block(&mut self, b: &Block, input: PipelineData) -> Result<PipelineData, ShellError> {
        let mut input = Some(input);
        let mut last = PipelineData::Empty;
        let n = b.stmts.len();
        for (idx, s) in b.stmts.iter().enumerate() {
            let inp = input.take().unwrap_or(PipelineData::Empty);
            let out = self.run_stmt(s, inp)?;
            if idx + 1 < n {
                drain(out)?;
            } else {
                last = out;
            }
        }
        Ok(last)
    }

    fn run_stmt(&mut self, s: &Stmt, input: PipelineData) -> Result<PipelineData, ShellError> {
        match s {
            Stmt::Let { name, value, span, .. } => {
                let v = self.run_pipeline(value, input)?.into_value(*span)?;
                self.set_var(name, v);
                Ok(PipelineData::Empty)
            }
            Stmt::Assign { name, op, value, span } => {
                let v = self.run_pipeline(value, input)?.into_value(*span)?;
                let v = match op {
                    Some(o) => {
                        let cur = self.lookup(name, *span)?;
                        binary_op(*o, &cur, &v, *span)?
                    }
                    None => v,
                };
                self.assign(name, v, *span)?;
                Ok(PipelineData::Empty)
            }
            Stmt::Def(d) => {
                self.g.defs.borrow_mut().insert(d.sig.name.clone(), d.clone());
                Ok(PipelineData::Empty)
            }
            Stmt::For { var, iter, body, .. } => {
                let it = self.eval_expr(iter, PipelineData::Empty)?;
                for item in it.into_iter_values(iter.span) {
                    let item = item?;
                    self.scopes.push(HashMap::new());
                    self.set_var(var, item);
                    let r = self.run_block(body, PipelineData::Empty).and_then(drain);
                    self.scopes.pop();
                    r?;
                }
                Ok(PipelineData::Empty)
            }
            Stmt::While { cond, body, .. } => {
                loop {
                    let c = self.eval_expr(cond, PipelineData::Empty)?.into_value(cond.span)?;
                    if !c.as_bool()? {
                        break;
                    }
                    self.scopes.push(HashMap::new());
                    let r = self.run_block(body, PipelineData::Empty).and_then(drain);
                    self.scopes.pop();
                    r?;
                }
                Ok(PipelineData::Empty)
            }
            Stmt::Pipeline(p) => self.run_pipeline(p, input),
            Stmt::Error(sp) => Err(ShellError::rt("cannot run code with parse errors", *sp)),
        }
    }

    pub fn run_pipeline(&mut self, p: &Pipeline, input: PipelineData) -> Result<PipelineData, ShellError> {
        let mut data = input;
        for e in &p.elements {
            data = self.eval_expr(e, data)?;
        }
        Ok(data)
    }

    /// Evaluate an expression that receives pipeline input.
    pub fn eval_expr(&mut self, e: &Expr, input: PipelineData) -> Result<PipelineData, ShellError> {
        match &e.kind {
            ExprKind::Call(c) => self.eval_call(c, e.span, input),
            ExprKind::External { head, args } => self.eval_external(head, args, e.span, input),
            ExprKind::Subexpr(b) => self.run_block(b, input),
            _ => {
                if !matches!(input, PipelineData::Empty) {
                    let v = input.into_value(e.span)?;
                    self.scopes.push(HashMap::from([("in".to_string(), v)]));
                    let r = self.eval(e);
                    self.scopes.pop();
                    return r.map(PipelineData::Value);
                }
                Ok(PipelineData::Value(self.eval(e)?))
            }
        }
    }

    /// Evaluate an expression to a value.
    pub fn eval(&mut self, e: &Expr) -> Result<Value, ShellError> {
        let sp = e.span;
        Ok(match &e.kind {
            ExprKind::Int(i) => Value::int(*i, sp),
            ExprKind::Float(f) => Value::float(*f, sp),
            ExprKind::Filesize(b) => Value::new(Val::Filesize(*b), sp),
            ExprKind::Duration(d) => Value::new(Val::Duration(*d), sp),
            ExprKind::Bool(b) => Value::bool(*b, sp),
            ExprKind::Nothing => Value::nothing(sp),
            ExprKind::String(s) => Value::string(s.clone(), sp),
            ExprKind::Interp(parts) => {
                let mut s = String::new();
                for p in parts {
                    let v = self.eval(p)?;
                    s.push_str(&v.to_text());
                }
                Value::string(s, sp)
            }
            ExprKind::Var(n) => {
                let mut v = self.lookup(n, sp)?;
                v.span = sp;
                v
            }
            ExprKind::Column(c) => {
                let it = self.lookup("it", sp)?;
                it.follow_path(&[PathMember::parse(c, sp)])?
            }
            ExprKind::Path { head, path } => {
                let h = self.eval(head)?;
                h.follow_path(path)?
            }
            ExprKind::List(items) => Value::list(items.iter().map(|x| self.eval(x)).collect::<Result<_, _>>()?, sp),
            ExprKind::Record(fields) => {
                let mut r: Record = Vec::new();
                for (k, v) in fields {
                    let k = self.eval(k)?.to_text();
                    let v = self.eval(v)?;
                    if let Some(slot) = r.iter_mut().find(|(x, _)| *x == k) {
                        slot.1 = v;
                    } else {
                        r.push((k, v));
                    }
                }
                Value::record(r, sp)
            }
            ExprKind::Closure { params, body } => Value::new(
                Val::Closure(Closure { params: params.clone(), body: body.clone(), captures: self.capture() }),
                sp,
            ),
            ExprKind::Range { from, to, inclusive } => {
                let start = self.eval(from)?.as_int()?;
                let end = match to {
                    Some(t) => Some(self.eval(t)?.as_int()?),
                    None => None,
                };
                Value::new(Val::Range(RangeVal { start, end, inclusive: *inclusive }), sp)
            }
            ExprKind::Binary { op: BinOp::And, lhs, rhs } => {
                let l = self.eval(lhs)?.as_bool()?;
                Value::bool(l && self.eval(rhs)?.as_bool()?, sp)
            }
            ExprKind::Binary { op: BinOp::Or, lhs, rhs } => {
                let l = self.eval(lhs)?.as_bool()?;
                Value::bool(l || self.eval(rhs)?.as_bool()?, sp)
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.eval(lhs)?;
                let r = self.eval(rhs)?;
                binary_op(*op, &l, &r, sp)?
            }
            ExprKind::Unary { op: UnOp::Not, expr } => Value::bool(!self.eval(expr)?.as_bool()?, sp),
            ExprKind::Unary { op: UnOp::Neg, expr } => {
                let v = self.eval(expr)?;
                match v.v {
                    Val::Int(i) => {
                        Value::int(i.checked_neg().ok_or_else(|| ShellError::rt("integer overflow", sp))?, sp)
                    }
                    Val::Float(f) => Value::float(-f, sp),
                    Val::Filesize(f) => Value::new(Val::Filesize(-f), sp),
                    Val::Duration(d) => Value::new(Val::Duration(-d), sp),
                    _ => return Err(ShellError::rt(format!("cannot negate {}", v.type_name()), sp)),
                }
            }
            ExprKind::Subexpr(b) => {
                self.scopes.push(HashMap::new());
                let r = self.run_block(b, PipelineData::Empty);
                self.scopes.pop();
                r?.into_value(sp)?
            }
            ExprKind::If { cond, then, else_ } => {
                let c = self.eval(cond)?;
                let c = c.as_bool().map_err(|_| {
                    ShellError::rt(format!("if condition must be bool, found {}", c.type_name()), cond.span)
                })?;
                if c {
                    self.scoped_block(then)?
                } else if let Some(e) = else_ {
                    self.eval(e)?
                } else {
                    Value::nothing(sp)
                }
            }
            ExprKind::Block(b) => self.scoped_block(b)?,
            ExprKind::Call(_) | ExprKind::External { .. } => self.eval_expr(e, PipelineData::Empty)?.into_value(sp)?,
            ExprKind::Garbage => return Err(ShellError::rt("cannot evaluate invalid expression", sp)),
        })
    }

    fn scoped_block(&mut self, b: &Block) -> Result<Value, ShellError> {
        self.scopes.push(HashMap::new());
        let r = self.run_block(b, PipelineData::Empty);
        self.scopes.pop();
        r?.into_value(b.span)
    }

    fn eval_call(&mut self, c: &Call, span: Span, input: PipelineData) -> Result<PipelineData, ShellError> {
        let mut args = CallArgs { positional: vec![], named: HashMap::new(), head: c.name_span, span };
        for a in &c.args {
            match a {
                Arg::Positional(e) => args.positional.push(self.eval(e)?),
                Arg::Named { name, value, .. } => {
                    let v = match value {
                        Some(e) => Some(self.eval(e)?),
                        None => None,
                    };
                    args.named.insert(name.clone(), v);
                }
            }
        }
        let def = self.g.defs.borrow().get(&c.name).cloned();
        if let Some(d) = def {
            return self.call_def(&d, args, input);
        }
        let g = self.g.clone();
        match g.builtins.get(&c.name) {
            Some(b) => {
                crate::commands::CURRENT_CMD.with(|n| n.replace(c.name.clone()));
                (b.run)(self, &args, input)
            }
            None => Err(ShellError::rt(format!("command '{}' not found", c.name), c.name_span)),
        }
    }

    fn call_def(&mut self, d: &Def, args: CallArgs, input: PipelineData) -> Result<PipelineData, ShellError> {
        let mut scope = HashMap::new();
        let mut pos = args.positional.into_iter();
        for p in &d.sig.positional {
            let v = match pos.next() {
                Some(v) => v,
                None => match &p.default {
                    Some(d) => d.clone(),
                    None if p.optional => Value::nothing(args.head),
                    None => {
                        return Err(ShellError::rt(format!("missing required argument '{}'", p.name), args.head));
                    }
                },
            };
            scope.insert(p.name.clone(), v);
        }
        if let Some(r) = &d.sig.rest {
            scope.insert(r.name.clone(), Value::list(pos.collect(), args.head));
        }
        for f in &d.sig.flags {
            let name = f.long.replace('-', "_");
            let v = match (&f.arg, args.named.get(&f.long)) {
                (None, x) => Value::bool(x.is_some(), args.head),
                (Some(_), Some(Some(v))) => v.clone(),
                _ => Value::nothing(args.head),
            };
            scope.insert(name, v);
        }
        let mut ev = Eval { g: self.g.clone(), captures: Rc::new(HashMap::new()), scopes: vec![scope] };
        let input = match input {
            PipelineData::Empty => PipelineData::Empty,
            other => {
                let v = other.into_value(args.span)?;
                ev.set_var("in", v.clone());
                PipelineData::Value(v)
            }
        };
        let out = ev.run_block(&d.body, input)?;
        // materialise so the callee's scope can be dropped safely
        Ok(match out {
            PipelineData::Bytes(b) => PipelineData::Bytes(b),
            other => PipelineData::Value(other.into_value(args.span)?),
        })
    }

    fn eval_external(
        &mut self,
        head: &Expr,
        args: &[Expr],
        span: Span,
        input: PipelineData,
    ) -> Result<PipelineData, ShellError> {
        let name = self.eval(head)?.to_text();
        let mut argv = Vec::new();
        for a in args {
            let v = self.eval(a)?;
            match &v.v {
                Val::List(l) => argv.extend(l.iter().map(|x| x.to_text())),
                _ => argv.push(v.to_text()),
            }
        }
        // glob expansion for simple `*` patterns
        let mut expanded = Vec::new();
        for a in argv {
            if a.contains('*') {
                let matches = crate::commands::glob(&self.g.cwd.borrow(), &a);
                if !matches.is_empty() {
                    expanded.extend(matches);
                    continue;
                }
            }
            expanded.push(a);
        }
        let mut cmd = Proc::new(&name);
        cmd.args(&expanded).current_dir(&*self.g.cwd.borrow());
        cmd.env_clear().envs(self.g.env.borrow().iter());
        cmd.stdout(Stdio::piped());
        let stdin_bytes: Option<Vec<u8>> = match input {
            PipelineData::Empty => None,
            PipelineData::Bytes(bs) => Some(bs.collect_bytes()?),
            PipelineData::Value(v) => Some(value_to_stdin(&v, self.g.width.get())),
            PipelineData::Stream(it, sp) => {
                Some(value_to_stdin(&Value::list(it.collect::<Result<_, _>>()?, sp), self.g.width.get()))
            }
        };
        cmd.stdin(if stdin_bytes.is_some() { Stdio::piped() } else { Stdio::inherit() });
        let mut child = cmd.spawn().map_err(|e| {
            let msg = if e.kind() == std::io::ErrorKind::NotFound {
                format!("command '{name}' not found")
            } else {
                format!("failed to run '{name}': {e}")
            };
            ShellError::rt(msg, head.span)
        })?;
        if let Some(bytes) = stdin_bytes {
            let mut stdin = child.stdin.take().unwrap();
            // write on a thread so a full stdout pipe cannot deadlock us
            std::thread::spawn(move || {
                let _ = stdin.write_all(&bytes);
            });
        }
        let stdout = child.stdout.take().unwrap();
        Ok(PipelineData::Bytes(ByteStream { reader: Box::new(stdout), child: Some(child), name, span }))
    }
}

/// Internal -> external auto-conversion: strings verbatim, lists one item per
/// line, everything else as a rendered table.
pub fn value_to_stdin(v: &Value, width: usize) -> Vec<u8> {
    match &v.v {
        Val::String(s) => s.clone().into_bytes(),
        Val::Binary(b) => b.clone(),
        Val::List(l) if l.iter().all(|x| !matches!(x.v, Val::Record(_) | Val::List(_))) => {
            let mut s = l.iter().map(|x| x.to_text()).collect::<Vec<_>>().join("\n");
            s.push('\n');
            s.into_bytes()
        }
        _ => crate::table::render(v, width).into_bytes(),
    }
}

/// Consume pipeline output for its side effects (externals still print).
pub fn drain(p: PipelineData) -> Result<(), ShellError> {
    match p {
        PipelineData::Stream(it, _) => {
            for x in it {
                x?;
            }
            Ok(())
        }
        PipelineData::Bytes(mut bs) => {
            let mut out = std::io::stdout();
            std::io::copy(&mut bs.reader, &mut out).map_err(|e| ShellError::rt(e.to_string(), bs.span))?;
            bs.finish()
        }
        _ => Ok(()),
    }
}

fn type_err(op: BinOp, l: &Value, r: &Value, span: Span) -> ShellError {
    ShellError::rt(
        format!("operator '{}' is not supported between {} and {}", op.as_str(), l.type_name(), r.type_name()),
        span,
    )
}

pub fn binary_op(op: BinOp, l: &Value, r: &Value, span: Span) -> Result<Value, ShellError> {
    use BinOp::*;
    use Val::*;
    let overflow = || ShellError::rt("integer overflow", span);
    let v = match (op, &l.v, &r.v) {
        (Add, Int(a), Int(b)) => Int(a.checked_add(*b).ok_or_else(overflow)?),
        (Sub, Int(a), Int(b)) => Int(a.checked_sub(*b).ok_or_else(overflow)?),
        (Mul, Int(a), Int(b)) => Int(a.checked_mul(*b).ok_or_else(overflow)?),
        (Add | Sub | Mul | Div | Pow, Int(_) | Float(_), Int(_) | Float(_))
            if matches!((&l.v, &r.v), (Float(_), _) | (_, Float(_))) || op == Div || op == Pow =>
        {
            let (a, b) = (l.as_f64().unwrap(), r.as_f64().unwrap());
            match op {
                Add => Float(a + b),
                Sub => Float(a - b),
                Mul => Float(a * b),
                Div => {
                    if b == 0.0 {
                        return Err(ShellError::rt("division by zero", r.span));
                    }
                    if let (Int(x), Int(y)) = (&l.v, &r.v)
                        && x % y == 0
                    {
                        Int(x / y)
                    } else {
                        Float(a / b)
                    }
                }
                _ => {
                    if let (Int(x), Int(y)) = (&l.v, &r.v)
                        && *y >= 0
                    {
                        Int(x.checked_pow(u32::try_from(*y).map_err(|_| overflow())?).ok_or_else(overflow)?)
                    } else {
                        Float(a.powf(b))
                    }
                }
            }
        }
        (FloorDiv, Int(a), Int(b)) => {
            if *b == 0 {
                return Err(ShellError::rt("division by zero", r.span));
            }
            Int(a.div_euclid(*b))
        }
        (FloorDiv, Int(_) | Float(_), Int(_) | Float(_)) => {
            let b = r.as_f64().unwrap();
            if b == 0.0 {
                return Err(ShellError::rt("division by zero", r.span));
            }
            Float((l.as_f64().unwrap() / b).floor())
        }
        (Mod, Int(a), Int(b)) => {
            if *b == 0 {
                return Err(ShellError::rt("division by zero", r.span));
            }
            Int(a.rem_euclid(*b))
        }
        (Mod, Int(_) | Float(_), Int(_) | Float(_)) => Float(l.as_f64().unwrap().rem_euclid(r.as_f64().unwrap())),
        (Add, Filesize(a), Filesize(b)) => Filesize(a + b),
        (Sub, Filesize(a), Filesize(b)) => Filesize(a - b),
        (Mul, Filesize(a), Int(b)) | (Mul, Int(b), Filesize(a)) => Filesize(a * b),
        (Div, Filesize(a), Int(b)) if *b != 0 => Filesize(a / b),
        (Add, Duration(a), Duration(b)) => Duration(a + b),
        (Sub, Duration(a), Duration(b)) => Duration(a - b),
        (Mul, Duration(a), Int(b)) | (Mul, Int(b), Duration(a)) => Duration(a * b),
        (Add, Date(d), Duration(n)) => Date(*d + chrono::Duration::nanoseconds(*n)),
        (Sub, Date(d), Duration(n)) => Date(*d - chrono::Duration::nanoseconds(*n)),
        (Sub, Date(a), Date(b)) => Duration((*a - *b).num_nanoseconds().unwrap_or(i64::MAX)),
        (Add | Concat, String(a), String(b)) => String(format!("{a}{b}")),
        (Concat, List(a), List(b)) => List(a.iter().chain(b).cloned().collect()),
        (Concat, List(a), _) => List(a.iter().cloned().chain(std::iter::once(r.clone())).collect()),
        (Eq, _, _) => Bool(values_equal(l, r)),
        (Ne, _, _) => Bool(!values_equal(l, r)),
        (Lt | Le | Gt | Ge, _, _) => {
            let comparable = matches!(
                (&l.v, &r.v),
                (Int(_) | Float(_), Int(_) | Float(_))
                    | (String(_), String(_))
                    | (Filesize(_), Filesize(_))
                    | (Duration(_), Duration(_))
                    | (Date(_), Date(_))
                    | (Bool(_), Bool(_))
            );
            if !comparable {
                return Err(type_err(op, l, r, span));
            }
            let o = compare_values(l, r);
            Bool(match op {
                Lt => o.is_lt(),
                Le => o.is_le(),
                Gt => o.is_gt(),
                _ => o.is_ge(),
            })
        }
        (RegexMatch | NotRegexMatch, String(a), String(b)) => {
            let re = regex::Regex::new(b).map_err(|e| ShellError::rt(format!("invalid regex: {e}"), r.span))?;
            Bool(re.is_match(a) == (op == RegexMatch))
        }
        (In | NotIn, _, _) => {
            let found = match (&l.v, &r.v) {
                (String(a), String(b)) => b.contains(a.as_str()),
                (_, List(items)) => items.iter().any(|x| values_equal(x, l)),
                (Int(i), Range(rg)) => match rg.end {
                    Some(e) => {
                        let (lo, hi) = (rg.start.min(e), rg.start.max(e));
                        *i >= lo && (*i < hi || (*i == hi && rg.inclusive))
                    }
                    None => *i >= rg.start,
                },
                (String(k), Record(rec)) => rec.iter().any(|(x, _)| x == k),
                _ => return Err(type_err(op, l, r, span)),
            };
            Bool(found == (op == In))
        }
        (StartsWith, String(a), String(b)) => Bool(a.starts_with(b.as_str())),
        (EndsWith, String(a), String(b)) => Bool(a.ends_with(b.as_str())),
        _ => return Err(type_err(op, l, r, span)),
    };
    Ok(Value::new(v, span))
}
