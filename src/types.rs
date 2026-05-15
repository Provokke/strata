//! Static types and command signatures.

use crate::value::Value;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    Any,
    Nothing,
    Bool,
    Int,
    Float,
    Number,
    String,
    Filesize,
    Duration,
    Date,
    Range,
    List(Box<Type>),
    Record,
    Table,
    Closure,
    Binary,
    Error,
    /// A `where`-style row condition argument.
    RowCondition,
    /// A cell path such as `a.0.b` (given as a bareword or string).
    CellPath,
}

impl Type {
    pub fn list(t: Type) -> Type {
        Type::List(Box::new(t))
    }

    pub fn from_name(s: &str) -> Option<Type> {
        Some(match s {
            "any" => Type::Any,
            "nothing" => Type::Nothing,
            "bool" => Type::Bool,
            "int" => Type::Int,
            "float" => Type::Float,
            "number" => Type::Number,
            "string" => Type::String,
            "filesize" => Type::Filesize,
            "duration" => Type::Duration,
            "date" => Type::Date,
            "range" => Type::Range,
            "list" => Type::list(Type::Any),
            "record" => Type::Record,
            "table" => Type::Table,
            "closure" => Type::Closure,
            "binary" => Type::Binary,
            "cell-path" => Type::CellPath,
            _ => {
                let inner = s.strip_prefix("list<")?.strip_suffix('>')?;
                return Type::from_name(inner).map(Type::list);
            }
        })
    }

    /// Is a value of type `self` acceptable where `expected` is required?
    /// `Any` on either side is accepted (gradual typing).
    pub fn is_subtype_of(&self, expected: &Type) -> bool {
        use Type::*;
        match (self, expected) {
            (Any, _) | (_, Any) => true,
            (a, b) if a == b => true,
            (Int | Float, Number) => true,
            (Number, Int | Float) => true,
            (Int, Float) => true,
            (Table, List(inner)) => matches!(**inner, Any | Record),
            (List(inner), Table) => matches!(**inner, Any | Record),
            (Range, List(inner)) => matches!(**inner, Any | Int | Number),
            (List(a), List(b)) => a.is_subtype_of(b),
            (String, CellPath) | (Int, CellPath) => true,
            (Closure, RowCondition) | (Bool, RowCondition) => true,
            _ => false,
        }
    }

    /// The element type when iterating a value of this type.
    pub fn element(&self) -> Type {
        match self {
            Type::List(t) => (**t).clone(),
            Type::Table => Type::Record,
            Type::Range => Type::Int,
            _ => Type::Any,
        }
    }

    pub fn of_value(v: &Value) -> Type {
        use crate::value::Val;
        match &v.v {
            Val::Nothing => Type::Nothing,
            Val::Bool(_) => Type::Bool,
            Val::Int(_) => Type::Int,
            Val::Float(_) => Type::Float,
            Val::String(_) => Type::String,
            Val::Filesize(_) => Type::Filesize,
            Val::Duration(_) => Type::Duration,
            Val::Date(_) => Type::Date,
            Val::Range(_) => Type::Range,
            Val::List(_) => Type::list(Type::Any),
            Val::Record(_) => Type::Record,
            Val::Closure(_) => Type::Closure,
            Val::Binary(_) => Type::Binary,
            Val::Error(_) => Type::Error,
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Type::Any => "any",
            Type::Nothing => "nothing",
            Type::Bool => "bool",
            Type::Int => "int",
            Type::Float => "float",
            Type::Number => "number",
            Type::String => "string",
            Type::Filesize => "filesize",
            Type::Duration => "duration",
            Type::Date => "date",
            Type::Range => "range",
            Type::List(t) => return write!(f, "list<{t}>"),
            Type::Record => "record",
            Type::Table => "table",
            Type::Closure => "closure",
            Type::Binary => "binary",
            Type::Error => "error",
            Type::RowCondition => "condition",
            Type::CellPath => "cell-path",
        };
        f.write_str(s)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PosArg {
    pub name: String,
    pub ty: Type,
    pub optional: bool,
    pub default: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FlagSpec {
    pub long: String,
    pub short: Option<char>,
    /// `None` for a boolean switch.
    pub arg: Option<Type>,
    pub desc: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Signature {
    pub name: String,
    pub desc: String,
    pub positional: Vec<PosArg>,
    pub rest: Option<PosArg>,
    pub flags: Vec<FlagSpec>,
    /// Accepted (input -> output) type pairs. Empty means any -> any.
    pub io: Vec<(Type, Type)>,
}

impl Signature {
    pub fn build(name: &str, desc: &str) -> Signature {
        Signature { name: name.into(), desc: desc.into(), ..Default::default() }
    }
    pub fn req(mut self, name: &str, ty: Type) -> Self {
        self.positional.push(PosArg { name: name.into(), ty, optional: false, default: None });
        self
    }
    pub fn opt(mut self, name: &str, ty: Type) -> Self {
        self.positional.push(PosArg { name: name.into(), ty, optional: true, default: None });
        self
    }
    pub fn rest(mut self, name: &str, ty: Type) -> Self {
        self.rest = Some(PosArg { name: name.into(), ty, optional: true, default: None });
        self
    }
    pub fn switch(mut self, long: &str, short: Option<char>, desc: &str) -> Self {
        self.flags.push(FlagSpec { long: long.into(), short, arg: None, desc: desc.into() });
        self
    }
    pub fn named(mut self, long: &str, short: Option<char>, ty: Type, desc: &str) -> Self {
        self.flags.push(FlagSpec { long: long.into(), short, arg: Some(ty), desc: desc.into() });
        self
    }
    pub fn io(mut self, i: Type, o: Type) -> Self {
        self.io.push((i, o));
        self
    }

    pub fn find_flag(&self, name: &str) -> Option<&FlagSpec> {
        if let Some(s) = name.strip_prefix('-') {
            let c = s.chars().next()?;
            self.flags.iter().find(|f| f.short == Some(c))
        } else {
            self.flags.iter().find(|f| f.long == name)
        }
    }

    /// Given an input type, compute the output type or `None` if not accepted.
    pub fn output_for(&self, input: &Type) -> Option<Type> {
        if self.io.is_empty() {
            return Some(Type::Any);
        }
        // exact match first, then subtype match
        for (i, o) in &self.io {
            if i == input {
                return Some(o.clone());
            }
        }
        for (i, o) in &self.io {
            if input.is_subtype_of(i) {
                if matches!(input, Type::Any) && self.io.len() > 1 {
                    return Some(Type::Any);
                }
                return Some(o.clone());
            }
        }
        None
    }

    pub fn usage(&self) -> String {
        let mut s = self.name.clone();
        for p in &self.positional {
            if p.optional {
                s.push_str(&format!(" ({}?: {})", p.name, p.ty));
            } else {
                s.push_str(&format!(" <{}: {}>", p.name, p.ty));
            }
        }
        if let Some(r) = &self.rest {
            s.push_str(&format!(" ...{}", r.name));
        }
        for f in &self.flags {
            match &f.arg {
                Some(t) => s.push_str(&format!(" --{} <{t}>", f.long)),
                None => s.push_str(&format!(" --{}", f.long)),
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtyping() {
        assert!(Type::Int.is_subtype_of(&Type::Number));
        assert!(Type::Table.is_subtype_of(&Type::list(Type::Any)));
        assert!(Type::Range.is_subtype_of(&Type::list(Type::Int)));
        assert!(!Type::String.is_subtype_of(&Type::Int));
        assert!(!Type::Record.is_subtype_of(&Type::list(Type::Any)));
        assert_eq!(Type::from_name("list<int>"), Some(Type::list(Type::Int)));
    }

    #[test]
    fn io_resolution() {
        let s = Signature::build("x", "")
            .io(Type::String, Type::String)
            .io(Type::list(Type::String), Type::list(Type::String));
        assert_eq!(s.output_for(&Type::String), Some(Type::String));
        assert_eq!(s.output_for(&Type::Int), None);
        assert_eq!(s.output_for(&Type::Any), Some(Type::Any));
    }
}
