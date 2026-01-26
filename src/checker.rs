//! Static type checker. Runs after parsing and before evaluation; infers
//! expression types and validates command arguments and pipeline input/output
//! types against signatures. Unknown types are `any` (gradual typing).

use crate::ast::*;
use crate::span::{ErrorKind, ShellError, Span};
use crate::types::{Signature, Type};
use std::collections::HashMap;

pub struct Checker<'a> {
    sigs: &'a HashMap<String, Signature>,
    scopes: Vec<HashMap<String, Type>>,
    pub errors: Vec<ShellError>,
}

pub fn check(block: &Block, sigs: &HashMap<String, Signature>, vars: &HashMap<String, Type>) -> Vec<ShellError> {
    let mut c = Checker { sigs, scopes: vec![vars.clone()], errors: vec![] };
    c.block(block, Type::Nothing);
    c.errors
}

fn err(msg: impl Into<String>, span: Span, label: impl Into<String>) -> ShellError {
    ShellError::new(ErrorKind::Type, msg, span, label)
}

fn is_num(t: &Type) -> bool {
    matches!(t, Type::Int | Type::Float | Type::Number)
}

impl<'a> Checker<'a> {
    fn bind(&mut self, name: &str, t: Type) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), t);
    }

    fn lookup(&self, name: &str) -> Option<Type> {
        for s in self.scopes.iter().rev() {
            if let Some(t) = s.get(name) {
                return Some(t.clone());
            }
        }
        match name {
            "env" => Some(Type::Record),
            "nothing" => Some(Type::Nothing),
            _ => None,
        }
    }

    fn scoped<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.scopes.push(HashMap::new());
        let r = f(self);
        self.scopes.pop();
        r
    }

    pub fn block(&mut self, b: &Block, input: Type) -> Type {
        let mut out = Type::Nothing;
        let mut input = Some(input);
        for s in &b.stmts {
            out = self.stmt(s, input.take().unwrap_or(Type::Nothing));
        }
        out
    }

    fn stmt(&mut self, s: &Stmt, input: Type) -> Type {
        match s {
            Stmt::Let { name, ty, value, .. } => {
                let t = self.pipeline(value, input);
                let bound = match ty {
                    Some(ann) => {
                        if !t.is_subtype_of(ann) {
                            self.errors.push(err(
                                format!("type mismatch: '{name}' is declared as {ann} but the value is {t}"),
                                value.span,
                                format!("expected {ann}, found {t}"),
                            ));
                        }
                        ann.clone()
                    }
                    None => t,
                };
                self.bind(name, bound);
                Type::Nothing
            }
            Stmt::Assign { name, value, span, op } => {
                let t = self.pipeline(value, input);
                match self.lookup(name) {
                    None => self.errors.push(err(format!("variable '${name}' not found"), *span, "not declared")),
                    Some(old) => {
                        if op.is_none() && !t.is_subtype_of(&old) && !old.is_subtype_of(&t) {
                            self.errors.push(err(
                                format!("cannot assign {t} to variable '${name}' of type {old}"),
                                value.span,
                                format!("expected {old}"),
                            ));
                        }
                    }
                }
                Type::Nothing
            }
            Stmt::Def(d) => {
                self.scopes.push(HashMap::new());
                for p in &d.sig.positional {
                    self.bind(&p.name, p.ty.clone());
                }
                if let Some(r) = &d.sig.rest {
                    self.bind(&r.name, Type::list(r.ty.clone()));
                }
                for f in &d.sig.flags {
                    let t = f.arg.clone().unwrap_or(Type::Bool);
                    self.bind(&f.long.replace('-', "_"), t);
                }
                let (inp, outp) = d.sig.io.first().cloned().unwrap_or((Type::Any, Type::Any));
                self.bind("in", inp.clone());
                let got = self.block(&d.body, inp);
                if !got.is_subtype_of(&outp) {
                    self.errors.push(err(
                        format!("command '{}' declares output {outp} but its body returns {got}", d.sig.name),
                        d.body.span,
                        format!("returns {got}"),
                    ));
                }
                self.scopes.pop();
                Type::Nothing
            }
            Stmt::For { var, iter, body, .. } => {
                let t = self.expr(iter, Type::Nothing);
                if matches!(t, Type::Int | Type::String | Type::Bool | Type::Float | Type::Record) {
                    self.errors.push(err(format!("cannot iterate over {t}"), iter.span, "not iterable"));
                }
                self.scoped(|c| {
                    c.bind(var, t.element());
                    c.block(body, Type::Nothing);
                });
                Type::Nothing
            }
            Stmt::While { cond, body, .. } => {
                let t = self.expr(cond, Type::Nothing);
                if !t.is_subtype_of(&Type::Bool) {
                    self.errors.push(err(format!("while condition must be bool, found {t}"), cond.span, "not a bool"));
                }
                self.scoped(|c| c.block(body, Type::Nothing));
                Type::Nothing
            }
            Stmt::Pipeline(p) => self.pipeline(p, input),
            Stmt::Error(_) => Type::Any,
        }
    }

    fn pipeline(&mut self, p: &Pipeline, input: Type) -> Type {
        let mut cur = input;
        for e in &p.elements {
            cur = self.element(e, cur);
        }
        cur
    }

    fn element(&mut self, e: &Expr, input: Type) -> Type {
        match &e.kind {
            ExprKind::Call(c) => self.call(c, e.span, input),
            ExprKind::External { head, args } => {
                self.expr(head, Type::Nothing);
                for a in args {
                    self.expr(a, Type::Nothing);
                }
                Type::Any
            }
            _ => self.scoped(|c| {
                if input != Type::Nothing {
                    c.bind("in", input.clone());
                }
                c.expr(e, input)
            }),
        }
    }

    fn call(&mut self, c: &Call, span: Span, input: Type) -> Type {
        let Some(sig) = self.sigs.get(&c.name).cloned() else {
            return Type::Any;
        };
        let elem = input.element();
        let mut npos = 0;
        for a in &c.args {
            match a {
                Arg::Positional(e) => {
                    let expected = match sig.positional.get(npos) {
                        Some(p) => Some(p.ty.clone()),
                        None => sig.rest.as_ref().map(|r| r.ty.clone()),
                    };
                    npos += 1;
                    let Some(expected) = expected else {
                        self.errors.push(err(
                            format!("extra positional argument for '{}'", c.name),
                            e.span,
                            "unexpected argument",
                        ));
                        self.expr(e, Type::Nothing);
                        continue;
                    };
                    let t = if let ExprKind::Closure { params, body } = &e.kind {
                        let elem = elem.clone();
                        self.scoped(|ck| {
                            for (i, p) in params.iter().enumerate() {
                                ck.bind(p, if i == 0 { elem.clone() } else { Type::Any });
                            }
                            ck.bind("in", elem.clone());
                            ck.bind("it", elem.clone());
                            ck.block(body, elem.clone());
                        });
                        Type::Closure
                    } else {
                        self.expr(e, Type::Nothing)
                    };
                    if !t.is_subtype_of(&expected) {
                        self.errors.push(err(
                            format!("type mismatch for argument of '{}': expected {expected}, found {t}", c.name),
                            e.span,
                            format!("expected {expected}"),
                        ));
                    }
                }
                Arg::Named { name, value, span } => {
                    if let Some(v) = value {
                        let t = self.expr(v, Type::Nothing);
                        if let Some(Some(exp)) = sig.find_flag(name).map(|f| f.arg.clone())
                            && !t.is_subtype_of(&exp)
                        {
                            self.errors.push(err(
                                format!("type mismatch for flag --{name}: expected {exp}, found {t}"),
                                v.span,
                                format!("expected {exp}"),
                            ));
                        }
                    } else {
                        let _ = span;
                    }
                }
            }
        }
        let required = sig.positional.iter().filter(|p| !p.optional).count();
        if npos < required {
            let missing = &sig.positional[npos];
            self.errors.push(
                err(
                    format!("missing required argument '{}' for '{}'", missing.name, c.name),
                    span,
                    format!("missing <{}: {}>", missing.name, missing.ty),
                )
                .with_help(format!("usage: {}", sig.usage())),
            );
        }
        match sig.output_for(&input) {
            Some(out) => {
                // filters keep the precise input type (list<string> stays list<string>)
                if sig.io.iter().any(|(i, o)| i == o && input.is_subtype_of(i))
                    && matches!(input, Type::List(_) | Type::Table)
                {
                    input
                } else {
                    out
                }
            }
            None => {
                let accepted: Vec<String> = sig.io.iter().map(|(i, _)| i.to_string()).collect();
                self.errors.push(
                    err(
                        format!("command '{}' does not accept {input} input", c.name),
                        c.name_span,
                        format!("does not accept {input}"),
                    )
                    .with_help(format!("'{}' accepts: {}", c.name, accepted.join(", "))),
                );
                Type::Any
            }
        }
    }

    pub fn expr(&mut self, e: &Expr, input: Type) -> Type {
        let sp = e.span;
        match &e.kind {
            ExprKind::Int(_) => Type::Int,
            ExprKind::Float(_) => Type::Float,
            ExprKind::Filesize(_) => Type::Filesize,
            ExprKind::Duration(_) => Type::Duration,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Nothing => Type::Nothing,
            ExprKind::String(_) => Type::String,
            ExprKind::Interp(parts) => {
                for p in parts {
                    self.expr(p, Type::Nothing);
                }
                Type::String
            }
            ExprKind::Var(n) => match self.lookup(n) {
                Some(t) => t,
                None if n == "in" || n == "it" => Type::Any,
                None => {
                    self.errors.push(err(format!("variable '${n}' not found"), sp, "undefined variable"));
                    Type::Any
                }
            },
            ExprKind::Column(_) => {
                if let Some(t) = self.lookup("it")
                    && matches!(t, Type::Int | Type::Float | Type::String | Type::Bool)
                {
                    self.errors.push(err(format!("cannot access a column of {t}"), sp, format!("rows are {t}")));
                }
                Type::Any
            }
            ExprKind::Path { head, path } => {
                let t = self.expr(head, input);
                if matches!(
                    t,
                    Type::Int
                        | Type::Float
                        | Type::Bool
                        | Type::String
                        | Type::Filesize
                        | Type::Duration
                        | Type::Nothing
                        | Type::Closure
                ) && !path.iter().all(|m| m.optional)
                {
                    let m = &path[0];
                    self.errors.push(err(
                        format!("cannot access '{}' on a value of type {t}", m.name),
                        m.span,
                        format!("{t} has no fields"),
                    ));
                }
                Type::Any
            }
            ExprKind::List(items) => {
                let ts: Vec<Type> = items.iter().map(|x| self.expr(x, Type::Nothing)).collect();
                if !ts.is_empty() && ts.iter().all(|t| *t == Type::Record) {
                    Type::Table
                } else if let Some(f) = ts.first()
                    && ts.iter().all(|t| t == f)
                {
                    Type::list(f.clone())
                } else {
                    Type::list(Type::Any)
                }
            }
            ExprKind::Record(fields) => {
                for (k, v) in fields {
                    self.expr(k, Type::Nothing);
                    self.expr(v, Type::Nothing);
                }
                Type::Record
            }
            ExprKind::Closure { params, body } => {
                self.scoped(|c| {
                    for p in params {
                        c.bind(p, Type::Any);
                    }
                    c.bind("in", Type::Any);
                    c.bind("it", Type::Any);
                    c.block(body, Type::Any);
                });
                Type::Closure
            }
            ExprKind::Range { from, to, .. } => {
                for x in std::iter::once(from).chain(to.iter()) {
                    let t = self.expr(x, Type::Nothing);
                    if !t.is_subtype_of(&Type::Int) || t == Type::Float {
                        self.errors.push(err(format!("range bounds must be int, found {t}"), x.span, "not an int"));
                    }
                }
                Type::Range
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.expr(lhs, input.clone());
                let r = self.expr(rhs, input);
                self.binary(*op, &l, &r, sp, lhs.span, rhs.span)
            }
            ExprKind::Unary { op, expr } => {
                let t = self.expr(expr, input);
                match op {
                    UnOp::Not => {
                        if !t.is_subtype_of(&Type::Bool) {
                            self.errors.push(err(format!("'not' expects bool, found {t}"), expr.span, "not a bool"));
                        }
                        Type::Bool
                    }
                    UnOp::Neg => {
                        if !(is_num(&t) || matches!(t, Type::Any | Type::Filesize | Type::Duration)) {
                            self.errors.push(err(format!("cannot negate {t}"), expr.span, "not a number"));
                            return Type::Any;
                        }
                        t
                    }
                }
            }
            ExprKind::Subexpr(b) => self.scoped(|c| c.block(b, Type::Nothing)),
            ExprKind::If { cond, then, else_ } => {
                let ct = self.expr(cond, Type::Nothing);
                if !ct.is_subtype_of(&Type::Bool) {
                    self.errors.push(err(format!("if condition must be bool, found {ct}"), cond.span, "not a bool"));
                }
                let a = self.scoped(|c| c.block(then, Type::Nothing));
                match else_ {
                    Some(e) => {
                        let b = self.expr(e, Type::Nothing);
                        if a == b { a } else { Type::Any }
                    }
                    None => Type::Any,
                }
            }
            ExprKind::Block(b) => self.scoped(|c| c.block(b, Type::Nothing)),
            ExprKind::Call(_) | ExprKind::External { .. } => self.element(e, Type::Nothing),
            ExprKind::Garbage => Type::Any,
        }
    }

    fn binary(&mut self, op: BinOp, l: &Type, r: &Type, sp: Span, lsp: Span, rsp: Span) -> Type {
        use BinOp::*;
        use Type::*;
        let any = matches!(l, Any) || matches!(r, Any);
        let bad = |c: &mut Self| {
            c.errors.push(
                err(
                    format!("operator '{}' is not supported between {l} and {r}", op.as_str()),
                    sp,
                    format!("{l} {} {r}", op.as_str()),
                )
                .with_help(if matches!(l, String) || matches!(r, String) {
                    "convert with 'into int' / 'into string' first".to_string()
                } else {
                    "check the operand types".to_string()
                }),
            );
            Type::Any
        };
        match op {
            Add | Sub | Mul | Div | FloorDiv | Mod | Pow => {
                if any {
                    return Type::Any;
                }
                match (l, r) {
                    (Int, Int) if op == Div || op == Pow => Number,
                    (Int, Int) => Int,
                    (a, b) if is_num(a) && is_num(b) => {
                        if matches!(op, FloorDiv) && *a == Int && *b == Int {
                            Int
                        } else if matches!(a, Float) || matches!(b, Float) {
                            Float
                        } else {
                            Number
                        }
                    }
                    (Filesize, Filesize) if matches!(op, Add | Sub) => Filesize,
                    (Filesize, Int) | (Int, Filesize) if op == Mul => Filesize,
                    (Filesize, Int) if op == Div => Filesize,
                    (Duration, Duration) if matches!(op, Add | Sub) => Duration,
                    (Duration, Int) | (Int, Duration) if op == Mul => Duration,
                    (Date, Duration) if matches!(op, Add | Sub) => Date,
                    (Date, Date) if op == Sub => Duration,
                    (String, String) if op == Add => String,
                    _ => bad(self),
                }
            }
            Concat => match (l, r) {
                _ if any => Type::Any,
                (String, String) => String,
                (List(_) | Table, _) => Type::list(Type::Any),
                _ => bad(self),
            },
            Eq | Ne => Bool,
            Lt | Le | Gt | Ge => {
                let ok = any
                    || (is_num(l) && is_num(r))
                    || (l == r && matches!(l, String | Filesize | Duration | Date | Bool));
                if !ok {
                    bad(self);
                }
                Bool
            }
            And | Or => {
                for (t, s) in [(l, lsp), (r, rsp)] {
                    if !t.is_subtype_of(&Bool) {
                        self.errors.push(err(
                            format!("'{}' expects bool operands, found {t}", op.as_str()),
                            s,
                            "not a bool",
                        ));
                    }
                }
                Bool
            }
            RegexMatch | NotRegexMatch | StartsWith | EndsWith => {
                if !(l.is_subtype_of(&String) && r.is_subtype_of(&String)) {
                    bad(self);
                }
                Bool
            }
            In | NotIn => {
                if matches!(r, Int | Float | Bool | Filesize | Duration | Nothing) {
                    bad(self);
                }
                Bool
            }
        }
    }
}
