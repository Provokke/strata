//! Abstract syntax tree and a canonical pretty-printer.

use crate::span::Span;
use crate::types::{Signature, Type};
use crate::value::PathMember;
use std::fmt::Write;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Let { name: String, mutable: bool, ty: Option<Type>, value: Pipeline, span: Span },
    Assign { name: String, op: Option<BinOp>, value: Pipeline, span: Span },
    Def(Rc<Def>),
    For { var: String, iter: Expr, body: Rc<Block>, span: Span },
    While { cond: Expr, body: Rc<Block>, span: Span },
    Pipeline(Pipeline),
    Error(Span),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Def {
    pub sig: Signature,
    pub body: Rc<Block>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pipeline {
    pub elements: Vec<Expr>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    RegexMatch,
    NotRegexMatch,
    In,
    NotIn,
    StartsWith,
    EndsWith,
    Concat,
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        use BinOp::*;
        match self {
            Or => "or",
            And => "and",
            Eq => "==",
            Ne => "!=",
            Lt => "<",
            Le => "<=",
            Gt => ">",
            Ge => ">=",
            RegexMatch => "=~",
            NotRegexMatch => "!~",
            In => "in",
            NotIn => "not-in",
            StartsWith => "starts-with",
            EndsWith => "ends-with",
            Concat => "++",
            Add => "+",
            Sub => "-",
            Mul => "*",
            Div => "/",
            FloorDiv => "//",
            Mod => "mod",
            Pow => "**",
        }
    }
    pub fn precedence(self) -> u8 {
        use BinOp::*;
        match self {
            Or => 1,
            And => 2,
            Eq | Ne | Lt | Le | Gt | Ge | RegexMatch | NotRegexMatch | In | NotIn | StartsWith | EndsWith => 3,
            Concat | Add | Sub => 4,
            Mul | Div | FloorDiv | Mod => 5,
            Pow => 6,
        }
    }
    pub fn all() -> &'static [BinOp] {
        use BinOp::*;
        &[
            Or,
            And,
            Eq,
            Ne,
            Lt,
            Le,
            Gt,
            Ge,
            RegexMatch,
            NotRegexMatch,
            In,
            NotIn,
            StartsWith,
            EndsWith,
            Concat,
            Add,
            Sub,
            Mul,
            Div,
            FloorDiv,
            Mod,
            Pow,
        ]
    }
    pub fn parse_op(s: &str) -> Option<BinOp> {
        BinOp::all().iter().copied().find(|o| o.as_str() == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Positional(Expr),
    Named { name: String, span: Span, value: Option<Expr> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub name: String,
    pub name_span: Span,
    pub args: Vec<Arg>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Filesize(i64),
    Duration(i64),
    Bool(bool),
    Nothing,
    String(String),
    Interp(Vec<Expr>),
    Var(String),
    /// Bare column name inside a `where` row condition (`$it.<name>`).
    Column(String),
    Path {
        head: Box<Expr>,
        path: Vec<PathMember>,
    },
    List(Vec<Expr>),
    Record(Vec<(Expr, Expr)>),
    Closure {
        params: Vec<String>,
        body: Rc<Block>,
    },
    Range {
        from: Box<Expr>,
        to: Option<Box<Expr>>,
        inclusive: bool,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    Subexpr(Rc<Block>),
    If {
        cond: Box<Expr>,
        then: Rc<Block>,
        else_: Option<Box<Expr>>,
    },
    Block(Rc<Block>),
    Call(Box<Call>),
    External {
        head: Box<Expr>,
        args: Vec<Expr>,
    },
    Garbage,
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Expr {
        Expr { kind, span }
    }
}

// ---------------------------------------------------------------------------
// Pretty printer (canonical form, used by round-trip property tests and `view`)

fn quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            '\r' => o.push_str("\\r"),
            '\0' => o.push_str("\\0"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub fn print_block(b: &Block) -> String {
    b.stmts.iter().map(print_stmt).collect::<Vec<_>>().join("\n")
}

pub fn print_stmt(s: &Stmt) -> String {
    match s {
        Stmt::Let { name, mutable, ty, value, .. } => {
            let kw = if *mutable { "mut" } else { "let" };
            let t = ty.as_ref().map(|t| format!(": {t}")).unwrap_or_default();
            format!("{kw} {name}{t} = {}", print_pipeline(value))
        }
        Stmt::Assign { name, op, value, .. } => {
            let o = op.map(|o| format!("{}=", o.as_str())).unwrap_or_else(|| "=".into());
            format!("${name} {o} {}", print_pipeline(value))
        }
        Stmt::Def(d) => {
            let mut params = Vec::new();
            for p in &d.sig.positional {
                params.push(format!("{}{}: {}", p.name, if p.optional { "?" } else { "" }, p.ty));
            }
            if let Some(r) = &d.sig.rest {
                params.push(format!("...{}: {}", r.name, r.ty));
            }
            for f in &d.sig.flags {
                let short = f.short.map(|c| format!("(-{c})")).unwrap_or_default();
                match &f.arg {
                    Some(t) => params.push(format!("--{}{short}: {t}", f.long)),
                    None => params.push(format!("--{}{short}", f.long)),
                }
            }
            format!("def {} [{}] {{ {} }}", quote(&d.sig.name), params.join(", "), print_block(&d.body))
        }
        Stmt::For { var, iter, body, .. } => format!("for {var} in {} {{ {} }}", print_expr(iter), print_block(body)),
        Stmt::While { cond, body, .. } => format!("while {} {{ {} }}", print_expr(cond), print_block(body)),
        Stmt::Pipeline(p) => print_pipeline(p),
        Stmt::Error(_) => "<error>".into(),
    }
}

pub fn print_pipeline(p: &Pipeline) -> String {
    p.elements.iter().map(print_expr).collect::<Vec<_>>().join(" | ")
}

fn print_path(path: &[PathMember]) -> String {
    let mut s = String::new();
    for m in path {
        let _ = write!(s, ".{}{}", m.name, if m.optional { "?" } else { "" });
    }
    s
}

pub fn print_expr(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Int(i) => i.to_string(),
        ExprKind::Float(f) => {
            let s = format!("{f:?}");
            if s.contains('.') || s.contains('e') { s } else { format!("{s}.0") }
        }
        ExprKind::Filesize(b) => format!("{b}b"),
        ExprKind::Duration(d) => format!("{d}ns"),
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Nothing => "null".into(),
        ExprKind::String(s) => quote(s),
        ExprKind::Interp(parts) => {
            let mut s = String::from("$\"");
            for p in parts {
                match &p.kind {
                    ExprKind::String(t) => {
                        for c in t.chars() {
                            match c {
                                '"' | '\\' | '(' | ')' => {
                                    s.push('\\');
                                    s.push(c)
                                }
                                '\n' => s.push_str("\\n"),
                                c => s.push(c),
                            }
                        }
                    }
                    ExprKind::Subexpr(_) => s.push_str(&print_expr(p)),
                    _ => {
                        let _ = write!(s, "({})", print_expr(p));
                    }
                }
            }
            s.push('"');
            s
        }
        ExprKind::Var(v) => format!("${v}"),
        ExprKind::Column(c) => c.clone(),
        ExprKind::Path { head, path } => match &head.kind {
            ExprKind::Var(v) => format!("${v}{}", print_path(path)),
            ExprKind::Subexpr(_) => format!("{}{}", print_expr(head), print_path(path)),
            _ => format!("({}){}", print_expr(head), print_path(path)),
        },
        ExprKind::List(items) => format!("[{}]", items.iter().map(print_expr).collect::<Vec<_>>().join(", ")),
        ExprKind::Record(fields) => {
            let f: Vec<String> = fields.iter().map(|(k, v)| format!("{}: {}", print_expr(k), print_expr(v))).collect();
            format!("{{{}}}", f.join(", "))
        }
        ExprKind::Closure { params, body } => {
            if params.is_empty() {
                format!("{{|| {}}}", print_block(body))
            } else {
                format!("{{|{}| {}}}", params.join(", "), print_block(body))
            }
        }
        ExprKind::Range { from, to, inclusive } => {
            let op = if *inclusive { ".." } else { "..<" };
            match to {
                Some(t) => format!("{}{op}{}", print_atom(from), print_atom(t)),
                None => format!("{}..", print_atom(from)),
            }
        }
        ExprKind::Binary { op, lhs, rhs } => {
            format!("{} {} {}", print_operand(lhs, *op, false), op.as_str(), print_operand(rhs, *op, true))
        }
        ExprKind::Unary { op: UnOp::Neg, expr } => match &expr.kind {
            ExprKind::Binary { .. } | ExprKind::Unary { op: UnOp::Not, .. } => format!("- ({})", print_expr(expr)),
            _ => format!("- {}", print_expr(expr)),
        },
        ExprKind::Unary { op: UnOp::Not, expr } => match &expr.kind {
            ExprKind::Binary { op, .. } if op.precedence() <= 2 => format!("not ({})", print_expr(expr)),
            _ => format!("not {}", print_expr(expr)),
        },
        ExprKind::Subexpr(b) => format!("({})", print_block(b)),
        ExprKind::If { cond, then, else_ } => {
            let mut s = format!("if {} {{ {} }}", print_expr(cond), print_block(then));
            if let Some(e) = else_ {
                let _ = write!(s, " else {}", print_expr(e));
            }
            s
        }
        ExprKind::Block(b) => format!("{{ {} }}", print_block(b)),
        ExprKind::Call(c) => {
            let mut s = c.name.clone();
            for a in &c.args {
                match a {
                    Arg::Positional(e) => {
                        let _ = write!(s, " {}", print_arg(e));
                    }
                    Arg::Named { name, value, .. } => {
                        let _ = write!(s, " --{name}");
                        if let Some(v) = value {
                            let _ = write!(s, " {}", print_arg(v));
                        }
                    }
                }
            }
            s
        }
        ExprKind::External { head, args } => {
            let mut s = format!("^{}", print_arg(head));
            for a in args {
                let _ = write!(s, " {}", print_arg(a));
            }
            s
        }
        ExprKind::Garbage => "<garbage>".into(),
    }
}

fn print_atom(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Int(i) if *i >= 0 => i.to_string(),
        ExprKind::Var(_) => print_expr(e),
        _ => format!("({})", print_expr(e)),
    }
}

/// Print a binary operand, adding parentheses only where precedence or
/// associativity requires them.
fn print_operand(e: &Expr, parent: BinOp, right: bool) -> String {
    let needs = match &e.kind {
        ExprKind::Binary { op, .. } => {
            let (p, q) = (op.precedence(), parent.precedence());
            p < q || (p == q && (right != (parent == BinOp::Pow)))
        }
        ExprKind::Unary { op: UnOp::Not, .. } => parent.precedence() >= 3,
        ExprKind::Call(_) | ExprKind::External { .. } => true,
        _ => false,
    };
    if needs { format!("({})", print_expr(e)) } else { print_expr(e) }
}

fn print_arg(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Binary { .. } | ExprKind::Unary { .. } | ExprKind::Call(_) | ExprKind::External { .. } => {
            format!("({})", print_expr(e))
        }
        ExprKind::If { .. } => format!("({})", print_expr(e)),
        _ => print_expr(e),
    }
}
