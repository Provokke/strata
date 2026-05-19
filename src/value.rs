//! The typed value model.

use crate::ast::Block;
use crate::span::{ShellError, Span};
use chrono::{DateTime, FixedOffset};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

pub type Record = Vec<(String, Value)>;

#[derive(Clone, Debug)]
pub struct Closure {
    pub params: Vec<String>,
    pub body: Rc<Block>,
    pub captures: Rc<HashMap<String, Value>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RangeVal {
    pub start: i64,
    pub end: Option<i64>,
    pub inclusive: bool,
}

impl RangeVal {
    pub fn iter(&self) -> Box<dyn Iterator<Item = i64>> {
        let start = self.start;
        match self.end {
            None => Box::new(start..),
            Some(e) if e >= start => {
                if self.inclusive {
                    Box::new(start..=e)
                } else {
                    Box::new(start..e)
                }
            }
            Some(e) => {
                let stop = if self.inclusive { e } else { e + 1 };
                Box::new((stop..=start).rev())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PathMember {
    pub name: String,
    pub index: Option<usize>,
    pub optional: bool,
    pub span: Span,
}

impl PathMember {
    pub fn parse(s: &str, span: Span) -> PathMember {
        let (s, optional) = match s.strip_suffix('?') {
            Some(t) => (t, true),
            None => (s, false),
        };
        PathMember { name: s.to_string(), index: s.parse().ok(), optional, span }
    }
}

#[derive(Clone, Debug)]
pub enum Val {
    Nothing,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    /// Bytes.
    Filesize(i64),
    /// Nanoseconds.
    Duration(i64),
    Date(DateTime<FixedOffset>),
    Range(RangeVal),
    List(Vec<Value>),
    Record(Record),
    Closure(Closure),
    Binary(Vec<u8>),
    Error(Box<ShellError>),
}

#[derive(Clone, Debug)]
pub struct Value {
    pub v: Val,
    pub span: Span,
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        values_equal(self, other)
    }
}

impl Value {
    pub fn new(v: Val, span: Span) -> Value {
        Value { v, span }
    }
    pub fn nothing(span: Span) -> Value {
        Value::new(Val::Nothing, span)
    }
    pub fn int(i: i64, span: Span) -> Value {
        Value::new(Val::Int(i), span)
    }
    pub fn float(f: f64, span: Span) -> Value {
        Value::new(Val::Float(f), span)
    }
    pub fn bool(b: bool, span: Span) -> Value {
        Value::new(Val::Bool(b), span)
    }
    pub fn string(s: impl Into<String>, span: Span) -> Value {
        Value::new(Val::String(s.into()), span)
    }
    pub fn list(l: Vec<Value>, span: Span) -> Value {
        Value::new(Val::List(l), span)
    }
    pub fn record(r: Record, span: Span) -> Value {
        Value::new(Val::Record(r), span)
    }

    pub fn type_name(&self) -> &'static str {
        match &self.v {
            Val::Nothing => "nothing",
            Val::Bool(_) => "bool",
            Val::Int(_) => "int",
            Val::Float(_) => "float",
            Val::String(_) => "string",
            Val::Filesize(_) => "filesize",
            Val::Duration(_) => "duration",
            Val::Date(_) => "date",
            Val::Range(_) => "range",
            Val::List(l) => {
                if !l.is_empty() && l.iter().all(|v| matches!(v.v, Val::Record(_))) {
                    "table"
                } else {
                    "list"
                }
            }
            Val::Record(_) => "record",
            Val::Closure(_) => "closure",
            Val::Binary(_) => "binary",
            Val::Error(_) => "error",
        }
    }

    /// Full type description, e.g. `table<name: string, age: int>`.
    pub fn describe(&self) -> String {
        match &self.v {
            Val::List(l) => {
                if l.is_empty() {
                    return "list<any>".into();
                }
                if let Val::Record(first) = &l[0].v
                    && l.iter().all(|v| matches!(v.v, Val::Record(_)))
                {
                    let cols: Vec<String> = first.iter().map(|(k, v)| format!("{k}: {}", v.describe())).collect();
                    return format!("table<{}>", cols.join(", "));
                }
                let first = l[0].describe();
                if l.iter().all(|v| v.describe() == first) { format!("list<{first}>") } else { "list<any>".into() }
            }
            Val::Record(r) => {
                let cols: Vec<String> = r.iter().map(|(k, v)| format!("{k}: {}", v.describe())).collect();
                format!("record<{}>", cols.join(", "))
            }
            _ => self.type_name().to_string(),
        }
    }

    pub fn as_str(&self) -> Result<&str, ShellError> {
        match &self.v {
            Val::String(s) => Ok(s),
            _ => Err(ShellError::rt(format!("expected string, found {}", self.type_name()), self.span)),
        }
    }
    pub fn as_int(&self) -> Result<i64, ShellError> {
        match &self.v {
            Val::Int(i) => Ok(*i),
            _ => Err(ShellError::rt(format!("expected int, found {}", self.type_name()), self.span)),
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match &self.v {
            Val::Int(i) => Some(*i as f64),
            Val::Float(f) => Some(*f),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Result<bool, ShellError> {
        match &self.v {
            Val::Bool(b) => Ok(*b),
            _ => Err(ShellError::rt(format!("expected bool, found {}", self.type_name()), self.span)),
        }
    }
    pub fn get_field(&self, name: &str) -> Option<&Value> {
        match &self.v {
            Val::Record(r) => r.iter().find(|(k, _)| k == name).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Follow a cell path (`a.0.b`).
    pub fn follow_path(&self, path: &[PathMember]) -> Result<Value, ShellError> {
        let mut cur = self.clone();
        for m in path {
            let next = match &cur.v {
                Val::List(items) => {
                    if let Some(i) = m.index {
                        items.get(i).cloned()
                    } else {
                        // column projection over a table
                        let mut out = Vec::new();
                        for it in items {
                            match it.get_field(&m.name) {
                                Some(v) => out.push(v.clone()),
                                None if m.optional => out.push(Value::nothing(m.span)),
                                None => {
                                    return Err(ShellError::rt(format!("column '{}' not found", m.name), m.span));
                                }
                            }
                        }
                        Some(Value::list(out, m.span))
                    }
                }
                Val::Range(r) => m.index.and_then(|i| r.iter().nth(i)).map(|i| Value::int(i, m.span)),
                Val::Record(r) => r.iter().find(|(k, _)| *k == m.name).map(|(_, v)| v.clone()),
                Val::Nothing if m.optional => Some(Value::nothing(m.span)),
                _ => {
                    return Err(ShellError::rt(
                        format!("cannot access '{}' on a value of type {}", m.name, cur.type_name()),
                        m.span,
                    ));
                }
            };
            match next {
                Some(v) => cur = v,
                None if m.optional => cur = Value::nothing(m.span),
                None => {
                    let what = if m.index.is_some() && matches!(cur.v, Val::List(_) | Val::Range(_)) {
                        format!("row {} not found", m.name)
                    } else {
                        format!("column '{}' not found", m.name)
                    };
                    return Err(ShellError::rt(what, m.span));
                }
            }
        }
        Ok(cur)
    }

    /// Replace (or insert, when `insert` is true) the value at a cell path.
    pub fn upsert_path(&mut self, path: &[PathMember], newv: Value, insert: bool) -> Result<(), ShellError> {
        let Some((m, rest)) = path.split_first() else {
            *self = newv;
            return Ok(());
        };
        let span = self.span;
        match &mut self.v {
            Val::Record(r) => {
                if let Some((_, v)) = r.iter_mut().find(|(k, _)| *k == m.name) {
                    if insert && rest.is_empty() {
                        return Err(ShellError::rt(format!("column '{}' already exists", m.name), m.span));
                    }
                    v.upsert_path(rest, newv, insert)
                } else if insert || rest.is_empty() {
                    if !insert {
                        return Err(ShellError::rt(format!("column '{}' not found", m.name), m.span));
                    }
                    let mut v = Value::record(vec![], span);
                    v.upsert_path(rest, newv, insert)?;
                    r.push((m.name.clone(), v));
                    Ok(())
                } else {
                    Err(ShellError::rt(format!("column '{}' not found", m.name), m.span))
                }
            }
            Val::List(items) => match m.index {
                Some(i) if i < items.len() => items[i].upsert_path(rest, newv, insert),
                _ => Err(ShellError::rt(format!("row {} not found", m.name), m.span)),
            },
            _ => Err(ShellError::rt(format!("cannot update '{}' on {}", m.name, self.type_name()), m.span)),
        }
    }

    /// Human-oriented single-line rendering (used in tables and `echo`).
    pub fn to_display(&self) -> String {
        match &self.v {
            Val::Nothing => String::new(),
            Val::Bool(b) => b.to_string(),
            Val::Int(i) => i.to_string(),
            Val::Float(f) => fmt_float(*f),
            Val::String(s) => s.clone(),
            Val::Filesize(b) => fmt_filesize(*b),
            Val::Duration(d) => fmt_duration(*d),
            Val::Date(d) => d.format("%Y-%m-%d %H:%M:%S %:z").to_string(),
            Val::Range(r) => match r.end {
                Some(e) => format!("{}..{}{}", r.start, if r.inclusive { "" } else { "<" }, e),
                None => format!("{}..", r.start),
            },
            Val::List(l) => {
                if l.is_empty() {
                    "[list 0 items]".into()
                } else if l.iter().all(|v| matches!(v.v, Val::Record(_))) {
                    format!("[table {} row{}]", l.len(), if l.len() == 1 { "" } else { "s" })
                } else {
                    format!("[list {} item{}]", l.len(), if l.len() == 1 { "" } else { "s" })
                }
            }
            Val::Record(r) => format!("{{record {} field{}}}", r.len(), if r.len() == 1 { "" } else { "s" }),
            Val::Closure(_) => "<closure>".into(),
            Val::Binary(b) => format!("[{} bytes]", b.len()),
            Val::Error(e) => format!("error: {}", e.msg),
        }
    }

    /// Plain text conversion used for `str` commands and external stdin.
    pub fn to_text(&self) -> String {
        match &self.v {
            Val::List(l) => l.iter().map(|v| v.to_text()).collect::<Vec<_>>().join("\n"),
            Val::Binary(b) => String::from_utf8_lossy(b).into_owned(),
            _ => self.to_display(),
        }
    }

    pub fn is_truthy(&self) -> bool {
        matches!(self.v, Val::Bool(true))
    }
}

pub fn fmt_float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{f:.1}")
    } else {
        let s = format!("{f}");
        s
    }
}

pub fn fmt_filesize(b: i64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = b as f64;
    let mut u = 0;
    while v.abs() >= 1024.0 && u < units.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{b} B") } else { format!("{v:.1} {}", units[u]) }
}

pub fn fmt_duration(ns: i64) -> String {
    let units: [(i64, &str); 7] = [
        (86_400_000_000_000 * 7, "wk"),
        (86_400_000_000_000, "day"),
        (3_600_000_000_000, "hr"),
        (60_000_000_000, "min"),
        (1_000_000_000, "sec"),
        (1_000_000, "ms"),
        (1_000, "us"),
    ];
    if ns == 0 {
        return "0sec".into();
    }
    let mut rem = ns.unsigned_abs() as i64;
    let mut parts = Vec::new();
    for (size, name) in units {
        if rem >= size {
            parts.push(format!("{}{}", rem / size, name));
            rem %= size;
        }
    }
    if rem > 0 {
        parts.push(format!("{rem}ns"));
    }
    let s = parts.join(" ");
    if ns < 0 { format!("-{s}") } else { s }
}

pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (&a.v, &b.v) {
        (Val::Nothing, Val::Nothing) => true,
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Int(x), Val::Int(y)) => x == y,
        (Val::Float(x), Val::Float(y)) => x == y,
        (Val::Int(x), Val::Float(y)) | (Val::Float(y), Val::Int(x)) => (*x as f64) == *y,
        (Val::String(x), Val::String(y)) => x == y,
        (Val::Filesize(x), Val::Filesize(y)) => x == y,
        (Val::Duration(x), Val::Duration(y)) => x == y,
        (Val::Date(x), Val::Date(y)) => x == y,
        (Val::Range(x), Val::Range(y)) => x == y,
        (Val::List(x), Val::List(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| values_equal(p, q)),
        (Val::Record(x), Val::Record(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|((k1, v1), (k2, v2))| k1 == k2 && values_equal(v1, v2))
        }
        (Val::Binary(x), Val::Binary(y)) => x == y,
        _ => false,
    }
}

fn type_rank(v: &Val) -> u8 {
    match v {
        Val::Bool(_) => 0,
        Val::Int(_) | Val::Float(_) => 1,
        Val::Filesize(_) => 2,
        Val::Duration(_) => 3,
        Val::Date(_) => 4,
        Val::String(_) => 5,
        Val::Range(_) => 6,
        Val::List(_) => 7,
        Val::Record(_) => 8,
        Val::Binary(_) => 9,
        Val::Closure(_) => 10,
        Val::Error(_) => 11,
        Val::Nothing => 12,
    }
}

/// Total order over values, used by sort-by, uniq, min/max.
pub fn compare_values(a: &Value, b: &Value) -> Ordering {
    match (&a.v, &b.v) {
        (Val::Bool(x), Val::Bool(y)) => x.cmp(y),
        (Val::Int(x), Val::Int(y)) => x.cmp(y),
        (Val::Float(_), _) | (_, Val::Float(_)) if a.as_f64().is_some() && b.as_f64().is_some() => {
            a.as_f64().unwrap().partial_cmp(&b.as_f64().unwrap()).unwrap_or(Ordering::Equal)
        }
        (Val::Filesize(x), Val::Filesize(y)) => x.cmp(y),
        (Val::Duration(x), Val::Duration(y)) => x.cmp(y),
        (Val::Date(x), Val::Date(y)) => x.cmp(y),
        (Val::String(x), Val::String(y)) => x.cmp(y),
        (Val::List(x), Val::List(y)) => {
            for (p, q) in x.iter().zip(y) {
                let o = compare_values(p, q);
                if o != Ordering::Equal {
                    return o;
                }
            }
            x.len().cmp(&y.len())
        }
        (Val::Record(x), Val::Record(y)) => {
            for ((k1, v1), (k2, v2)) in x.iter().zip(y) {
                let o = k1.cmp(k2).then_with(|| compare_values(v1, v2));
                if o != Ordering::Equal {
                    return o;
                }
            }
            x.len().cmp(&y.len())
        }
        _ => type_rank(&a.v).cmp(&type_rank(&b.v)),
    }
}

/// Convert a JSON value into a strata value.
pub fn from_json(j: &serde_json::Value, span: Span) -> Value {
    match j {
        serde_json::Value::Null => Value::nothing(span),
        serde_json::Value::Bool(b) => Value::bool(*b, span),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Value::int(i, span),
            None => Value::float(n.as_f64().unwrap_or(0.0), span),
        },
        serde_json::Value::String(s) => Value::string(s.clone(), span),
        serde_json::Value::Array(a) => Value::list(a.iter().map(|x| from_json(x, span)).collect(), span),
        serde_json::Value::Object(o) => {
            Value::record(o.iter().map(|(k, v)| (k.clone(), from_json(v, span))).collect(), span)
        }
    }
}

pub fn to_json(v: &Value) -> Result<serde_json::Value, ShellError> {
    Ok(match &v.v {
        Val::Nothing => serde_json::Value::Null,
        Val::Bool(b) => serde_json::Value::Bool(*b),
        Val::Int(i) | Val::Filesize(i) | Val::Duration(i) => serde_json::Value::from(*i),
        Val::Float(f) => {
            serde_json::Number::from_f64(*f).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null)
        }
        Val::String(s) => serde_json::Value::String(s.clone()),
        Val::Date(d) => serde_json::Value::String(d.to_rfc3339()),
        Val::Range(r) => serde_json::Value::Array(r.iter().take(1_000_000).map(serde_json::Value::from).collect()),
        Val::List(l) => serde_json::Value::Array(l.iter().map(to_json).collect::<Result<_, _>>()?),
        Val::Record(r) => {
            let mut m = serde_json::Map::new();
            for (k, x) in r {
                m.insert(k.clone(), to_json(x)?);
            }
            serde_json::Value::Object(m)
        }
        Val::Binary(b) => serde_json::Value::Array(b.iter().map(|x| serde_json::Value::from(*x)).collect()),
        Val::Closure(_) | Val::Error(_) => {
            return Err(ShellError::rt(format!("cannot convert {} to json", v.type_name()), v.span));
        }
    })
}

pub fn from_toml(t: &toml::Value, span: Span) -> Value {
    match t {
        toml::Value::String(s) => Value::string(s.clone(), span),
        toml::Value::Integer(i) => Value::int(*i, span),
        toml::Value::Float(f) => Value::float(*f, span),
        toml::Value::Boolean(b) => Value::bool(*b, span),
        toml::Value::Datetime(d) => match DateTime::parse_from_rfc3339(&d.to_string()) {
            Ok(dt) => Value::new(Val::Date(dt), span),
            Err(_) => Value::string(d.to_string(), span),
        },
        toml::Value::Array(a) => Value::list(a.iter().map(|x| from_toml(x, span)).collect(), span),
        toml::Value::Table(t) => Value::record(t.iter().map(|(k, v)| (k.clone(), from_toml(v, span))).collect(), span),
    }
}

pub fn to_toml(v: &Value) -> Result<toml::Value, ShellError> {
    Ok(match &v.v {
        Val::Bool(b) => toml::Value::Boolean(*b),
        Val::Int(i) | Val::Filesize(i) | Val::Duration(i) => toml::Value::Integer(*i),
        Val::Float(f) => toml::Value::Float(*f),
        Val::String(s) => toml::Value::String(s.clone()),
        Val::Date(d) => toml::Value::String(d.to_rfc3339()),
        Val::List(l) => toml::Value::Array(l.iter().map(to_toml).collect::<Result<_, _>>()?),
        Val::Record(r) => {
            let mut m = toml::map::Map::new();
            for (k, x) in r {
                if matches!(x.v, Val::Nothing) {
                    continue;
                }
                m.insert(k.clone(), to_toml(x)?);
            }
            toml::Value::Table(m)
        }
        _ => return Err(ShellError::rt(format!("cannot convert {} to toml", v.type_name()), v.span)),
    })
}

/// Infer a typed scalar from text (used by `from csv`).
pub fn infer_scalar(s: &str, span: Span) -> Value {
    if let Ok(i) = s.parse::<i64>() {
        Value::int(i, span)
    } else if let Ok(f) = s.parse::<f64>()
        && s.chars().any(|c| c.is_ascii_digit())
    {
        Value::float(f, span)
    } else if s == "true" || s == "false" {
        Value::bool(s == "true", span)
    } else {
        Value::string(s, span)
    }
}
