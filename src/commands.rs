//! Builtin commands.

use crate::eval::{Builtin, BuiltinFn, CallArgs, Eval, PipelineData, ValueIter};
use crate::span::{ShellError, Span};
use crate::types::{Signature, Type};
use crate::value::*;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::Path;

type R = Result<PipelineData, ShellError>;

fn ok(v: Value) -> R {
    Ok(PipelineData::Value(v))
}

fn list_t() -> Type {
    Type::list(Type::Any)
}

fn table_io(s: Signature) -> Signature {
    s.io(Type::Table, Type::Table).io(list_t(), list_t())
}

pub fn builtins() -> HashMap<String, Builtin> {
    use Type as T;
    let mut m = HashMap::new();
    let mut add = |sig: Signature, run: BuiltinFn| {
        m.insert(sig.name.clone(), Builtin { sig, run });
    };
    add(
        Signature::build("ls", "List directory contents")
            .opt("pattern", T::String)
            .switch("all", Some('a'), "include hidden")
            .io(T::Nothing, T::Table),
        ls,
    );
    add(Signature::build("ps", "List processes").io(T::Nothing, T::Table), ps);
    add(
        Signature::build("open", "Open a file, parsing json/csv/toml by extension")
            .req("path", T::String)
            .switch("raw", Some('r'), "do not parse")
            .io(T::Nothing, T::Any),
        open,
    );
    add(
        Signature::build("save", "Save pipeline input to a file")
            .req("path", T::String)
            .switch("force", Some('f'), "overwrite")
            .io(T::Any, T::Nothing),
        save,
    );
    add(
        Signature::build("to json", "Convert to JSON text")
            .switch("raw", Some('r'), "compact output")
            .io(T::Any, T::String),
        to_json_cmd,
    );
    add(Signature::build("to csv", "Convert a table to CSV text").io(T::Any, T::String), to_csv);
    add(Signature::build("to toml", "Convert a record to TOML text").io(T::Record, T::String), to_toml_cmd);
    add(Signature::build("to text", "Convert to plain text").io(T::Any, T::String), to_text);
    add(Signature::build("from json", "Parse JSON text").io(T::String, T::Any), from_json_cmd);
    add(
        Signature::build("from csv", "Parse CSV text into a table")
            .switch("noheaders", Some('n'), "no header row")
            .io(T::String, T::Table),
        from_csv,
    );
    add(Signature::build("from toml", "Parse TOML text").io(T::String, T::Record), from_toml_cmd);
    add(table_io(Signature::build("where", "Filter rows by a condition").req("condition", T::RowCondition)), where_cmd);
    add(table_io(Signature::build("filter", "Filter items with a closure").req("closure", T::Closure)), where_cmd);
    add(
        Signature::build("select", "Select columns")
            .rest("columns", T::CellPath)
            .io(T::Record, T::Record)
            .io(T::Table, T::Table)
            .io(list_t(), list_t()),
        select,
    );
    add(
        Signature::build("reject", "Remove columns")
            .rest("columns", T::CellPath)
            .io(T::Record, T::Record)
            .io(T::Table, T::Table)
            .io(list_t(), list_t()),
        reject,
    );
    add(
        Signature::build("get", "Extract data at a cell path")
            .req("path", T::CellPath)
            .io(T::Record, T::Any)
            .io(list_t(), T::Any),
        get,
    );
    add(
        table_io(Signature::build("sort-by", "Sort by columns").rest("columns", T::CellPath).switch(
            "reverse",
            Some('r'),
            "descending",
        )),
        sort_by,
    );
    add(table_io(Signature::build("sort", "Sort values").switch("reverse", Some('r'), "descending")), sort_by);
    add(
        Signature::build("group-by", "Group rows by a column").req("column", T::CellPath).io(list_t(), T::Record),
        group_by,
    );
    add(Signature::build("first", "First item(s)").opt("n", T::Int).io(list_t(), T::Any), first);
    add(Signature::build("last", "Last item(s)").opt("n", T::Int).io(list_t(), T::Any), last);
    add(table_io(Signature::build("skip", "Skip n items").opt("n", T::Int)), skip);
    add(table_io(Signature::build("take", "Take n items").req("n", T::Int)), take);
    add(Signature::build("length", "Count items").io(list_t(), T::Int), length);
    add(Signature::build("each", "Run a closure on each item").req("closure", T::Closure).io(list_t(), list_t()), each);
    add(
        Signature::build("reduce", "Aggregate a list with a closure |it, acc|")
            .req("closure", T::Closure)
            .named("fold", Some('f'), T::Any, "initial value")
            .io(list_t(), T::Any),
        reduce,
    );
    for (n, d) in [("math sum", "Sum"), ("math avg", "Average"), ("math min", "Minimum"), ("math max", "Maximum")] {
        add(Signature::build(n, d).io(list_t(), T::Any), math);
    }
    add(Signature::build("math round", "Round numbers").io(T::Number, T::Int).io(list_t(), list_t()), math_round);
    let strio = |s: Signature| s.io(T::String, T::String).io(T::list(T::String), T::list(T::String));
    add(strio(Signature::build("str upcase", "Uppercase")), str_map);
    add(strio(Signature::build("str downcase", "Lowercase")), str_map);
    add(strio(Signature::build("str trim", "Trim whitespace")), str_map);
    add(
        Signature::build("str length", "String length").io(T::String, T::Int).io(T::list(T::String), T::list(T::Int)),
        str_map,
    );
    add(
        Signature::build("str contains", "Substring test")
            .req("substring", T::String)
            .switch("ignore-case", Some('i'), "case-insensitive")
            .io(T::String, T::Bool)
            .io(T::list(T::String), T::list(T::Bool)),
        str_map,
    );
    add(
        strio(Signature::build("str replace", "Replace text").req("find", T::String).req("replace", T::String).switch(
            "all",
            Some('a'),
            "replace all",
        )),
        str_map,
    );
    add(
        Signature::build("str split", "Split a string").req("separator", T::String).io(T::String, T::list(T::String)),
        str_split,
    );
    add(
        Signature::build("split row", "Split a string into rows")
            .req("separator", T::String)
            .io(T::String, T::list(T::String)),
        str_split,
    );
    add(
        Signature::build("str join", "Join a list of strings").opt("separator", T::String).io(list_t(), T::String),
        str_join,
    );
    add(
        Signature::build("lines", "Split text into lines")
            .io(T::String, T::list(T::String))
            .io(T::Any, T::list(T::String)),
        lines,
    );
    add(
        Signature::build("split column", "Split strings into columns")
            .req("separator", T::String)
            .rest("names", T::String)
            .io(T::String, T::Table)
            .io(T::list(T::String), T::Table),
        split_column,
    );
    add(
        Signature::build("update", "Update a column")
            .req("field", T::CellPath)
            .req("value", T::Any)
            .io(T::Record, T::Record)
            .io(T::Table, T::Table),
        update,
    );
    add(
        Signature::build("insert", "Insert a column")
            .req("field", T::CellPath)
            .req("value", T::Any)
            .io(T::Record, T::Record)
            .io(T::Table, T::Table),
        update,
    );
    add(
        Signature::build("rename", "Rename columns positionally")
            .rest("names", T::String)
            .io(T::Record, T::Record)
            .io(T::Table, T::Table),
        rename,
    );
    add(table_io(Signature::build("uniq", "Remove duplicates")), uniq);
    add(table_io(Signature::build("reverse", "Reverse order")), reverse);
    add(Signature::build("flatten", "Flatten nested lists/records").io(list_t(), list_t()), flatten);
    add(
        Signature::build("range", "Generate a lazy range, or select rows by index range")
            .req("range", T::Range)
            .io(T::Nothing, T::list(T::Int))
            .io(list_t(), list_t()),
        range,
    );
    add(Signature::build("describe", "Describe the type of the input").io(T::Any, T::String), describe);
    add(Signature::build("echo", "Return arguments").rest("values", T::Any).io(T::Nothing, T::Any), echo);
    add(Signature::build("cd", "Change directory").opt("path", T::String).io(T::Nothing, T::Nothing), cd);
    add(Signature::build("pwd", "Current directory").io(T::Nothing, T::String), pwd);
    add(Signature::build("env", "Environment variables").io(T::Nothing, T::Table), env);
    add(
        Signature::build("help", "Show help").opt("command", T::String).rest("more", T::String).io(T::Nothing, T::Any),
        help,
    );
    add(
        Signature::build("table", "Render as a table string")
            .named("width", Some('w'), T::Int, "width")
            .io(T::Any, T::String),
        table,
    );
    add(Signature::build("enumerate", "Add an index column").io(list_t(), T::Table), enumerate);
    add(Signature::build("wrap", "Wrap values into a column").req("name", T::String).io(T::Any, T::Any), wrap);
    add(table_io(Signature::build("append", "Append a value").req("value", T::Any)), append);
    add(table_io(Signature::build("prepend", "Prepend a value").req("value", T::Any)), append);
    add(table_io(Signature::build("compact", "Remove null items")), compact);
    add(Signature::build("into int", "Convert to int").io(T::Any, T::Int), into);
    add(Signature::build("into string", "Convert to string").io(T::Any, T::String), into);
    add(Signature::build("into float", "Convert to float").io(T::Any, T::Float), into);
    add(Signature::build("is-empty", "Is the input empty?").io(T::Any, T::Bool), is_empty);
    add(Signature::build("ignore", "Discard input").io(T::Any, T::Nothing), ignore);
    add(
        Signature::build("columns", "Column names").io(T::Record, T::list(T::String)).io(T::Table, T::list(T::String)),
        columns,
    );
    add(Signature::build("error make", "Raise an error").req("message", T::String).io(T::Any, T::Nothing), error_make);
    m
}

// ---------------------------------------------------------------------------
// helpers

fn cell_path(v: &Value) -> Result<Vec<PathMember>, ShellError> {
    match &v.v {
        Val::String(s) => {
            let mut off = v.span.start;
            Ok(s.split('.')
                .map(|p| {
                    let m = PathMember::parse(p, Span::new(off, off + p.len()));
                    off += p.len() + 1;
                    m
                })
                .collect())
        }
        Val::Int(i) => {
            Ok(vec![PathMember { name: i.to_string(), index: Some(*i as usize), optional: false, span: v.span }])
        }
        _ => Err(ShellError::rt(format!("expected cell path, found {}", v.type_name()), v.span)),
    }
}

fn input_list(input: PipelineData, span: Span) -> Result<Vec<Value>, ShellError> {
    input.into_iter_values(span).collect()
}

fn need_record_or_list(v: &Value, cmd: &str) -> Result<(), ShellError> {
    match v.v {
        Val::Record(_) | Val::List(_) => Ok(()),
        _ => Err(ShellError::rt(format!("{cmd} expects a record or table, found {}", v.type_name()), v.span)),
    }
}

/// Apply `f` to a record, or lazily to every row of a list/stream.
fn map_rows(input: PipelineData, span: Span, f: impl Fn(Value) -> Result<Value, ShellError> + 'static) -> R {
    match input {
        PipelineData::Value(v) if matches!(v.v, Val::Record(_)) => ok(f(v)?),
        other => {
            let it = other.into_iter_values(span);
            Ok(PipelineData::stream(it.map(move |x| x.and_then(&f)), span))
        }
    }
}

pub fn glob(cwd: &Path, pattern: &str) -> Vec<String> {
    let (dir, pat) = match pattern.rfind('/') {
        Some(i) => (&pattern[..i + 1], &pattern[i + 1..]),
        None => ("", pattern),
    };
    let base = if dir.is_empty() { cwd.to_path_buf() } else { cwd.join(dir) };
    let mut out: Vec<String> = match std::fs::read_dir(&base) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| wildcard(pat, n) && (pat.starts_with('.') || !n.starts_with('.')))
            .map(|n| format!("{dir}{n}"))
            .collect(),
        Err(_) => vec![],
    };
    out.sort();
    out
}

fn wildcard(p: &str, s: &str) -> bool {
    let (p, s): (Vec<char>, Vec<char>) = (p.chars().collect(), s.chars().collect());
    let (mut pi, mut si, mut star, mut mark) = (0, 0, None, 0);
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(st) = star {
            pi = st + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

// ---------------------------------------------------------------------------
// filesystem & system

fn file_entry(path: &Path, display: String, span: Span) -> Value {
    let md = std::fs::symlink_metadata(path);
    let (ty, size, modified) = match &md {
        Ok(m) => {
            let ty = if m.file_type().is_symlink() {
                "symlink"
            } else if m.is_dir() {
                "dir"
            } else {
                "file"
            };
            let modified = m.modified().ok().map(|t| {
                let dt: chrono::DateTime<chrono::Local> = t.into();
                Value::new(Val::Date(dt.fixed_offset()), span)
            });
            (ty, m.len() as i64, modified)
        }
        Err(_) => ("unknown", 0, None),
    };
    Value::record(
        vec![
            ("name".into(), Value::string(display, span)),
            ("type".into(), Value::string(ty, span)),
            ("size".into(), Value::new(Val::Filesize(size), span)),
            ("modified".into(), modified.unwrap_or(Value::nothing(span))),
        ],
        span,
    )
}

fn ls(ev: &mut Eval, a: &CallArgs, _i: PipelineData) -> R {
    let cwd = ev.g.cwd.borrow().clone();
    let all = a.has("all");
    let mut out = Vec::new();
    match a.opt(0) {
        Some(p) => {
            let pat = p.as_str()?.to_string();
            if pat.contains('*') || pat.contains('?') {
                for n in glob(&cwd, &pat) {
                    out.push(file_entry(&cwd.join(&n), n, a.span));
                }
            } else {
                let full = ev.g.resolve(&pat);
                if full.is_dir() {
                    let mut names = read_names(&full, all, p.span)?;
                    names.sort();
                    for n in names {
                        let shown = Path::new(&pat).join(&n).to_string_lossy().into_owned();
                        out.push(file_entry(&full.join(&n), shown, a.span));
                    }
                } else if full.exists() {
                    out.push(file_entry(&full, pat, a.span));
                } else {
                    return Err(ShellError::rt(format!("no such file or directory: {pat}"), p.span));
                }
            }
        }
        None => {
            let mut names = read_names(&cwd, all, a.head)?;
            names.sort();
            for n in names {
                out.push(file_entry(&cwd.join(&n), n, a.span));
            }
        }
    }
    ok(Value::list(out, a.span))
}

fn read_names(dir: &Path, all: bool, span: Span) -> Result<Vec<String>, ShellError> {
    let rd = std::fs::read_dir(dir).map_err(|e| ShellError::rt(format!("cannot read directory: {e}"), span))?;
    Ok(rd
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| all || !n.starts_with('.'))
        .collect())
}

fn ps(_ev: &mut Eval, a: &CallArgs, _i: PipelineData) -> R {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_processes();
    let mut rows: Vec<(u32, Value)> = sys
        .processes()
        .values()
        .map(|p| {
            let pid = p.pid().as_u32();
            let r = Value::record(
                vec![
                    ("pid".into(), Value::int(pid as i64, a.span)),
                    ("name".into(), Value::string(p.name().to_string(), a.span)),
                    ("cpu".into(), Value::float(p.cpu_usage() as f64, a.span)),
                    ("mem".into(), Value::new(Val::Filesize(p.memory() as i64), a.span)),
                ],
                a.span,
            );
            (pid, r)
        })
        .collect();
    rows.sort_by_key(|(p, _)| *p);
    ok(Value::list(rows.into_iter().map(|(_, r)| r).collect(), a.span))
}

fn ext_of(p: &str) -> String {
    Path::new(p).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn parse_format(fmt: &str, text: &str, span: Span) -> Result<Option<Value>, ShellError> {
    Ok(Some(match fmt {
        "json" => parse_json(text, span)?,
        "csv" => parse_csv(text, true, span)?,
        "toml" => parse_toml(text, span)?,
        _ => return Ok(None),
    }))
}

fn open(ev: &mut Eval, a: &CallArgs, _i: PipelineData) -> R {
    let p = a.req(0)?;
    let name = p.as_str()?;
    let path = ev.g.resolve(name);
    let bytes = std::fs::read(&path).map_err(|e| ShellError::rt(format!("cannot open '{name}': {e}"), p.span))?;
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(e) => return ok(Value::new(Val::Binary(e.into_bytes()), a.span)),
    };
    if !a.has("raw")
        && let Some(v) = parse_format(&ext_of(name), &text, a.span)?
    {
        return ok(v);
    }
    ok(Value::string(text, a.span))
}

fn save(ev: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let p = a.req(0)?;
    let name = p.as_str()?;
    let path = ev.g.resolve(name);
    if path.exists() && !a.has("force") {
        return Err(ShellError::rt(format!("file '{name}' already exists (use --force to overwrite)"), p.span));
    }
    let bytes = match input {
        PipelineData::Bytes(b) => b.collect_bytes()?,
        other => {
            let v = other.into_value(a.span)?;
            match (&v.v, ext_of(name).as_str()) {
                (Val::String(s), _) => s.clone().into_bytes(),
                (Val::Binary(b), _) => b.clone(),
                (_, "json") => json_text(&v, false)?.into_bytes(),
                (_, "csv") => csv_text(&v)?.into_bytes(),
                (_, "toml") => toml_text(&v)?.into_bytes(),
                _ => v.to_text().into_bytes(),
            }
        }
    };
    std::fs::write(&path, bytes).map_err(|e| ShellError::rt(format!("cannot write '{name}': {e}"), p.span))?;
    Ok(PipelineData::Empty)
}

// ---------------------------------------------------------------------------
// formats

pub fn parse_json(text: &str, span: Span) -> Result<Value, ShellError> {
    let j: serde_json::Value =
        serde_json::from_str(text).map_err(|e| ShellError::rt(format!("invalid json: {e}"), span))?;
    Ok(from_json(&j, span))
}

pub fn parse_csv(text: &str, headers: bool, span: Span) -> Result<Value, ShellError> {
    let mut rdr = csv::ReaderBuilder::new().has_headers(headers).flexible(true).from_reader(text.as_bytes());
    let hdr: Vec<String> = if headers {
        rdr.headers()
            .map_err(|e| ShellError::rt(format!("invalid csv: {e}"), span))?
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        vec![]
    };
    let mut rows = Vec::new();
    for rec in rdr.records() {
        let rec = rec.map_err(|e| ShellError::rt(format!("invalid csv: {e}"), span))?;
        let r: Record = rec
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let k = hdr.get(i).cloned().unwrap_or_else(|| format!("column{}", i + 1));
                (k, infer_scalar(f, span))
            })
            .collect();
        rows.push(Value::record(r, span));
    }
    Ok(Value::list(rows, span))
}

pub fn parse_toml(text: &str, span: Span) -> Result<Value, ShellError> {
    let t: toml::Value = toml::from_str(text).map_err(|e| ShellError::rt(format!("invalid toml: {e}"), span))?;
    Ok(from_toml(&t, span))
}

fn json_text(v: &Value, raw: bool) -> Result<String, ShellError> {
    let j = to_json(v)?;
    Ok(if raw { serde_json::to_string(&j).unwrap() } else { serde_json::to_string_pretty(&j).unwrap() })
}

fn csv_text(v: &Value) -> Result<String, ShellError> {
    let rows: Vec<Value> = match &v.v {
        Val::List(l) => l.clone(),
        Val::Record(_) => vec![v.clone()],
        _ => return Err(ShellError::rt(format!("to csv expects a table, found {}", v.type_name()), v.span)),
    };
    let mut cols: Vec<String> = Vec::new();
    for r in &rows {
        match &r.v {
            Val::Record(rec) => {
                for (k, _) in rec {
                    if !cols.contains(k) {
                        cols.push(k.clone());
                    }
                }
            }
            _ => return Err(ShellError::rt(format!("to csv expects records, found {}", r.type_name()), r.span)),
        }
    }
    let mut w = csv::Writer::from_writer(vec![]);
    w.write_record(&cols).map_err(|e| ShellError::rt(e.to_string(), v.span))?;
    for r in &rows {
        let fields: Vec<String> =
            cols.iter().map(|c| r.get_field(c).map(|x| x.to_text()).unwrap_or_default()).collect();
        w.write_record(&fields).map_err(|e| ShellError::rt(e.to_string(), v.span))?;
    }
    Ok(String::from_utf8(w.into_inner().unwrap()).unwrap())
}

fn toml_text(v: &Value) -> Result<String, ShellError> {
    if !matches!(v.v, Val::Record(_)) {
        return Err(ShellError::rt(format!("to toml expects a record, found {}", v.type_name()), v.span));
    }
    toml::to_string(&to_toml(v)?).map_err(|e| ShellError::rt(e.to_string(), v.span))
}

fn text_input(input: PipelineData, span: Span, cmd: &str) -> Result<(String, Span), ShellError> {
    let v = input.into_value(span)?;
    match v.v {
        Val::String(s) => Ok((s, v.span)),
        Val::Binary(b) => Ok((String::from_utf8_lossy(&b).into_owned(), v.span)),
        _ => Err(ShellError::rt(format!("{cmd} expects string input, found {}", v.type_name()), v.span)),
    }
}

fn to_json_cmd(_: &mut Eval, a: &CallArgs, i: PipelineData) -> R {
    let v = i.into_value(a.span)?;
    ok(Value::string(json_text(&v, a.has("raw"))?, a.span))
}
fn to_csv(_: &mut Eval, a: &CallArgs, i: PipelineData) -> R {
    ok(Value::string(csv_text(&i.into_value(a.span)?)?, a.span))
}
fn to_toml_cmd(_: &mut Eval, a: &CallArgs, i: PipelineData) -> R {
    ok(Value::string(toml_text(&i.into_value(a.span)?)?, a.span))
}
fn to_text(_: &mut Eval, a: &CallArgs, i: PipelineData) -> R {
    ok(Value::string(i.into_value(a.span)?.to_text(), a.span))
}
fn from_json_cmd(_: &mut Eval, a: &CallArgs, i: PipelineData) -> R {
    let (t, sp) = text_input(i, a.span, "from json")?;
    ok(parse_json(&t, sp)?)
}
fn from_csv(_: &mut Eval, a: &CallArgs, i: PipelineData) -> R {
    let (t, sp) = text_input(i, a.span, "from csv")?;
    ok(parse_csv(&t, !a.has("noheaders"), sp)?)
}
fn from_toml_cmd(_: &mut Eval, a: &CallArgs, i: PipelineData) -> R {
    let (t, sp) = text_input(i, a.span, "from toml")?;
    ok(parse_toml(&t, sp)?)
}

// ---------------------------------------------------------------------------
// filters

fn where_cmd(ev: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let run = ev.runner(a.closure(0)?);
    let it = input.into_iter_values(a.span);
    let out = it.filter_map(move |x| match x {
        Err(e) => Some(Err(e)),
        Ok(v) => match run.call(v.clone(), &[]) {
            Ok(b) => match b.v {
                Val::Bool(true) => Some(Ok(v)),
                Val::Bool(false) => None,
                _ => Some(Err(ShellError::rt(format!("condition must return bool, found {}", b.type_name()), b.span))),
            },
            Err(e) => Some(Err(e)),
        },
    });
    Ok(PipelineData::stream(out, a.span))
}

fn each(ev: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let run = ev.runner(a.closure(0)?);
    let it = input.into_iter_values(a.span);
    Ok(PipelineData::stream(it.map(move |x| x.and_then(|v| run.call(v, &[]))), a.span))
}

fn reduce(ev: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let run = ev.runner(a.closure(0)?);
    let mut it = input.into_iter_values(a.span);
    let mut acc = match a.flag("fold") {
        Some(v) => v.clone(),
        None => match it.next() {
            Some(v) => v?,
            None => return Err(ShellError::rt("reduce on empty input requires --fold", a.head)),
        },
    };
    for x in it {
        acc = run.call(x?, std::slice::from_ref(&acc))?;
    }
    ok(acc)
}

fn select_one(v: &Value, paths: &[Vec<PathMember>], names: &[String]) -> Result<Value, ShellError> {
    let mut r = Vec::new();
    for (p, n) in paths.iter().zip(names) {
        r.push((n.clone(), v.follow_path(p)?));
    }
    Ok(Value::record(r, v.span))
}

fn select(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let paths: Vec<Vec<PathMember>> = a.positional.iter().map(cell_path).collect::<Result<_, _>>()?;
    let names: Vec<String> = a.positional.iter().map(|v| v.to_text()).collect();
    map_rows(input, a.span, move |row| {
        need_record_or_list(&row, "select")?;
        select_one(&row, &paths, &names)
    })
}

fn reject(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let names: Vec<String> = a.positional.iter().map(|v| v.to_text()).collect();
    map_rows(input, a.span, move |row| match row.v {
        Val::Record(r) => Ok(Value::record(r.into_iter().filter(|(k, _)| !names.contains(k)).collect(), row.span)),
        _ => Err(ShellError::rt(format!("reject expects a record or table, found {}", row.type_name()), row.span)),
    })
}

fn get(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let p = cell_path(a.req(0)?)?;
    let v = input.into_value(a.span)?;
    ok(v.follow_path(&p)?)
}

fn sort_by(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let mut items = input_list(input, a.span)?;
    let paths: Vec<Vec<PathMember>> = a.positional.iter().map(cell_path).collect::<Result<_, _>>()?;
    if paths.is_empty() {
        items.sort_by(compare_values);
    } else {
        let mut keyed: Vec<(Vec<Value>, Value)> = items
            .into_iter()
            .map(|it| Ok((paths.iter().map(|p| it.follow_path(p)).collect::<Result<Vec<_>, _>>()?, it)))
            .collect::<Result<_, ShellError>>()?;
        keyed.sort_by(|(ka, _), (kb, _)| {
            ka.iter()
                .zip(kb)
                .map(|(x, y)| compare_values(x, y))
                .find(|o| o.is_ne())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        items = keyed.into_iter().map(|(_, v)| v).collect();
    }
    if a.has("reverse") {
        items.reverse();
    }
    ok(Value::list(items, a.span))
}

fn group_by(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let p = cell_path(a.req(0)?)?;
    let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
    for it in input.into_iter_values(a.span) {
        let it = it?;
        let k = it.follow_path(&p)?.to_display();
        match groups.iter_mut().find(|(g, _)| *g == k) {
            Some((_, v)) => v.push(it),
            None => groups.push((k, vec![it])),
        }
    }
    ok(Value::record(groups.into_iter().map(|(k, v)| (k, Value::list(v, a.span))).collect(), a.span))
}

fn count_arg(a: &CallArgs, i: usize, default: i64) -> Result<usize, ShellError> {
    match a.opt(i) {
        Some(v) => {
            let n = v.as_int()?;
            usize::try_from(n).map_err(|_| ShellError::rt("expected a non-negative count", v.span))
        }
        None => Ok(default as usize),
    }
}

fn first(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    if let PipelineData::Value(v) = &input
        && matches!(v.v, Val::String(_) | Val::Record(_) | Val::Int(_))
    {
        return Err(ShellError::rt(format!("first expects a list, found {}", v.type_name()), v.span));
    }
    let mut it = input.into_iter_values(a.span);
    if a.opt(0).is_none() {
        return match it.next() {
            Some(v) => ok(v?),
            None => Err(ShellError::rt("first: input is empty", a.head)),
        };
    }
    let n = count_arg(a, 0, 1)?;
    ok(Value::list(it.take(n).collect::<Result<_, _>>()?, a.span))
}

fn last(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let n = count_arg(a, 0, 1)?;
    let mut buf = std::collections::VecDeque::new();
    for x in input.into_iter_values(a.span) {
        buf.push_back(x?);
        if buf.len() > n {
            buf.pop_front();
        }
    }
    if a.opt(0).is_none() {
        return buf.pop_back().map(PipelineData::Value).ok_or_else(|| ShellError::rt("last: input is empty", a.head));
    }
    ok(Value::list(buf.into_iter().collect(), a.span))
}

fn skip(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let n = count_arg(a, 0, 1)?;
    Ok(PipelineData::stream(input.into_iter_values(a.span).skip(n), a.span))
}

fn take(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let n = count_arg(a, 0, 1)?;
    Ok(PipelineData::stream(input.into_iter_values(a.span).take(n), a.span))
}

fn length(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let n = match input {
        PipelineData::Value(v) => match &v.v {
            Val::List(l) => l.len(),
            Val::Binary(b) => b.len(),
            Val::Range(_) => value_iter_count(v)?,
            Val::Nothing => 0,
            _ => return Err(ShellError::rt(format!("length expects a list, found {}", v.type_name()), v.span)),
        },
        other => {
            let mut c = 0;
            for x in other.into_iter_values(a.span) {
                x?;
                c += 1;
            }
            c
        }
    };
    ok(Value::int(n as i64, a.span))
}

fn value_iter_count(v: Value) -> Result<usize, ShellError> {
    if let Val::Range(r) = &v.v
        && r.end.is_none()
    {
        return Err(ShellError::rt("cannot take the length of an unbounded range", v.span));
    }
    Ok(crate::eval::value_iter(v).count())
}

// ---------------------------------------------------------------------------
// math

fn math(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let items = input_list(input, a.span)?;
    let name = a.head;
    let which = math_kind();
    match which {
        "min" | "max" => {
            let mut best: Option<Value> = None;
            for v in items {
                best = Some(match best {
                    None => v,
                    Some(b) => {
                        let o = compare_values(&v, &b);
                        if (which == "min" && o.is_lt()) || (which == "max" && o.is_gt()) { v } else { b }
                    }
                });
            }
            best.map(PipelineData::Value).ok_or_else(|| ShellError::rt(format!("math {which}: input is empty"), name))
        }
        _ => {
            let mut acc = Value::int(0, a.span);
            let n = items.len();
            for v in &items {
                if !matches!(v.v, Val::Int(_) | Val::Float(_) | Val::Filesize(_) | Val::Duration(_)) {
                    return Err(ShellError::rt(
                        format!("math {which}: expected a number, found {}", v.type_name()),
                        v.span,
                    ));
                }
                acc = if matches!(acc.v, Val::Int(0)) && !matches!(v.v, Val::Int(_) | Val::Float(_)) {
                    v.clone()
                } else {
                    crate::eval::binary_op(crate::ast::BinOp::Add, &acc, v, a.span)?
                };
            }
            if which == "avg" {
                if n == 0 {
                    return Err(ShellError::rt("math avg: input is empty", name));
                }
                acc = match acc.v {
                    Val::Filesize(b) => Value::new(Val::Filesize(b / n as i64), a.span),
                    Val::Duration(d) => Value::new(Val::Duration(d / n as i64), a.span),
                    _ => Value::float(acc.as_f64().unwrap_or(0.0) / n as f64, a.span),
                };
            }
            acc.span = a.span;
            ok(acc)
        }
    }
}

/// Builtins share one fn per family; dispatch on the running subcommand.
fn math_kind() -> &'static str {
    CURRENT_CMD.with(|c| match c.borrow().as_str() {
        "math avg" => "avg",
        "math min" => "min",
        "math max" => "max",
        _ => "sum",
    })
}

thread_local! {
    /// Name of the builtin currently executing (set by the evaluator), so a
    /// single function can implement a family of subcommands.
    pub static CURRENT_CMD: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

pub fn current_cmd() -> String {
    CURRENT_CMD.with(|c| c.borrow().clone())
}

fn math_round(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let f = |v: Value| -> Result<Value, ShellError> {
        match v.v {
            Val::Float(x) => Ok(Value::int(x.round() as i64, v.span)),
            Val::Int(_) => Ok(v),
            _ => Err(ShellError::rt(format!("math round: expected a number, found {}", v.type_name()), v.span)),
        }
    };
    match input {
        PipelineData::Value(v) if !matches!(v.v, Val::List(_) | Val::Range(_)) => ok(f(v)?),
        other => Ok(PipelineData::stream(other.into_iter_values(a.span).map(move |x| x.and_then(f)), a.span)),
    }
}

// ---------------------------------------------------------------------------
// strings

fn str_map(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let cmd = current_cmd();
    let args: Vec<String> = a.positional.iter().map(|v| v.to_text()).collect();
    let all = a.has("all");
    let icase = a.has("ignore-case");
    let f = move |v: Value| -> Result<Value, ShellError> {
        let s = match &v.v {
            Val::String(s) => s,
            _ => return Err(ShellError::rt(format!("{cmd} expects string input, found {}", v.type_name()), v.span)),
        };
        Ok(match cmd.as_str() {
            "str upcase" => Value::string(s.to_uppercase(), v.span),
            "str downcase" => Value::string(s.to_lowercase(), v.span),
            "str trim" => Value::string(s.trim(), v.span),
            "str length" => Value::int(s.chars().count() as i64, v.span),
            "str contains" => {
                if icase {
                    Value::bool(s.to_lowercase().contains(&args[0].to_lowercase()), v.span)
                } else {
                    Value::bool(s.contains(&args[0]), v.span)
                }
            }
            "str replace" => {
                if all {
                    Value::string(s.replace(&args[0], &args[1]), v.span)
                } else {
                    Value::string(s.replacen(&args[0], &args[1], 1), v.span)
                }
            }
            other => return Err(ShellError::rt(format!("unknown string command {other}"), v.span)),
        })
    };
    match input {
        PipelineData::Value(v) if matches!(v.v, Val::String(_)) => ok(f(v)?),
        PipelineData::Value(v) if !matches!(v.v, Val::List(_) | Val::Range(_)) => ok(f(v)?),
        PipelineData::Bytes(b) => ok(f(PipelineData::Bytes(b).into_value(a.span)?)?),
        other => Ok(PipelineData::stream(other.into_iter_values(a.span).map(move |x| x.and_then(&f)), a.span)),
    }
}

fn str_split(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let sep = a.req(0)?.to_text();
    let (s, sp) = text_input(input, a.span, "split")?;
    let parts = s.split(sep.as_str()).map(|p| Value::string(p, sp)).collect();
    ok(Value::list(parts, a.span))
}

fn str_join(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let sep = a.opt(0).map(|v| v.to_text()).unwrap_or_default();
    let items = input_list(input, a.span)?;
    ok(Value::string(items.iter().map(|v| v.to_text()).collect::<Vec<_>>().join(&sep), a.span))
}

fn lines(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let span = a.span;
    match input {
        // lazy line splitting of external output
        PipelineData::Bytes(mut bs) => {
            let reader = BufReader::new(std::mem::replace(&mut bs.reader, Box::new(std::io::empty())));
            let mut bs = Some(bs);
            let mut lines = reader.lines();
            let it = std::iter::from_fn(move || match lines.next() {
                Some(Ok(l)) => Some(Ok(Value::string(l.trim_end_matches('\r'), span))),
                Some(Err(e)) => Some(Err(ShellError::rt(e.to_string(), span))),
                None => bs.take().and_then(|mut b| b.finish().err()).map(Err),
            });
            Ok(PipelineData::stream(it, span))
        }
        other => {
            let it = other.into_iter_values(span);
            let out = it.flat_map(move |x| -> ValueIter {
                match x {
                    Err(e) => Box::new(std::iter::once(Err(e))),
                    Ok(v) => match v.v {
                        Val::String(s) => {
                            let ls: Vec<_> = s.lines().map(|l| Ok(Value::string(l, v.span))).collect();
                            Box::new(ls.into_iter())
                        }
                        _ => Box::new(std::iter::once(Err(ShellError::rt(
                            format!("lines expects string input, found {}", v.type_name()),
                            v.span,
                        )))),
                    },
                }
            });
            Ok(PipelineData::stream(out, span))
        }
    }
}

fn split_column(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let sep = a.req(0)?.to_text();
    let names: Vec<String> = a.positional[1..].iter().map(|v| v.to_text()).collect();
    let f = move |v: Value| -> Result<Value, ShellError> {
        let s = v.as_str()?;
        let parts: Vec<&str> =
            if sep.trim().is_empty() { s.split_whitespace().collect() } else { s.split(sep.as_str()).collect() };
        let r = parts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (names.get(i).cloned().unwrap_or_else(|| format!("column{}", i + 1)), Value::string(p.trim(), v.span))
            })
            .collect();
        Ok(Value::record(r, v.span))
    };
    match input {
        PipelineData::Value(v) if matches!(v.v, Val::String(_)) => ok(Value::list(vec![f(v)?], a.span)),
        other => Ok(PipelineData::stream(other.into_iter_values(a.span).map(move |x| x.and_then(&f)), a.span)),
    }
}

// ---------------------------------------------------------------------------
// structure

fn update(ev: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let insert = current_cmd() == "insert";
    let path = cell_path(a.req(0)?)?;
    let newv = a.req(1)?.clone();
    let runner = match &newv.v {
        Val::Closure(c) => Some(ev.runner(c.clone())),
        _ => None,
    };
    map_rows(input, a.span, move |mut row| {
        need_record_or_list(&row, if insert { "insert" } else { "update" })?;
        let v = match &runner {
            Some(r) => {
                let cur = if insert { row.clone() } else { row.follow_path(&path)? };
                r.call(cur, &[])?
            }
            None => newv.clone(),
        };
        if !insert {
            row.follow_path(&path)?;
        }
        row.upsert_path(&path, v, insert)?;
        Ok(row)
    })
}

fn rename(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let names: Vec<String> = a.positional.iter().map(|v| v.to_text()).collect();
    map_rows(input, a.span, move |row| match row.v {
        Val::Record(r) => Ok(Value::record(
            r.into_iter().enumerate().map(|(i, (k, v))| (names.get(i).cloned().unwrap_or(k), v)).collect(),
            row.span,
        )),
        _ => Err(ShellError::rt(format!("rename expects a record or table, found {}", row.type_name()), row.span)),
    })
}

fn uniq(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for x in input.into_iter_values(a.span) {
        let x = x?;
        let key = to_json(&x).map(|j| format!("{}:{j}", x.type_name())).unwrap_or_else(|_| x.to_display());
        if seen.insert(key) {
            out.push(x);
        }
    }
    ok(Value::list(out, a.span))
}

fn reverse(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let mut l = input_list(input, a.span)?;
    l.reverse();
    ok(Value::list(l, a.span))
}

fn flatten(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let mut out = Vec::new();
    for x in input.into_iter_values(a.span) {
        let x = x?;
        match x.v {
            Val::List(inner) => out.extend(inner),
            Val::Record(r) if r.iter().any(|(_, v)| matches!(v.v, Val::Record(_))) => {
                let mut flat = Vec::new();
                for (k, v) in r {
                    match v.v {
                        Val::Record(inner) => flat.extend(inner),
                        _ => flat.push((k, v)),
                    }
                }
                out.push(Value::record(flat, x.span));
            }
            _ => out.push(x),
        }
    }
    ok(Value::list(out, a.span))
}

fn range(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let rv = a.req(0)?;
    let Val::Range(r) = &rv.v else {
        return Err(ShellError::rt(format!("expected range, found {}", rv.type_name()), rv.span));
    };
    let span = a.span;
    match input {
        PipelineData::Empty => {
            let it = r.iter().map(move |i| Ok(Value::int(i, span)));
            Ok(PipelineData::stream(it, span))
        }
        other => {
            let (start, end) = (r.start.max(0) as usize, r.end);
            let incl = r.inclusive;
            let it = other.into_iter_values(span).skip(start);
            match end {
                Some(e) => {
                    let n = (e as usize + usize::from(incl)).saturating_sub(start);
                    Ok(PipelineData::stream(it.take(n), span))
                }
                None => Ok(PipelineData::stream(it, span)),
            }
        }
    }
}

fn describe(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let d = match input {
        PipelineData::Bytes(_) => "byte stream".to_string(),
        other => other.into_value(a.span)?.describe(),
    };
    ok(Value::string(d, a.span))
}

fn echo(_: &mut Eval, a: &CallArgs, _i: PipelineData) -> R {
    match a.positional.len() {
        0 => Ok(PipelineData::Empty),
        1 => ok(a.positional[0].clone()),
        _ => ok(Value::list(a.positional.clone(), a.span)),
    }
}

fn cd(ev: &mut Eval, a: &CallArgs, _i: PipelineData) -> R {
    let target = match a.opt(0) {
        Some(p) => ev.g.resolve(p.as_str()?),
        None => match ev.g.env.borrow().get("HOME").or(ev.g.env.borrow().get("USERPROFILE")) {
            Some(h) => h.into(),
            None => return Err(ShellError::rt("cannot determine home directory", a.head)),
        },
    };
    let canon = std::fs::canonicalize(&target)
        .map_err(|e| ShellError::rt(format!("cannot cd to '{}': {e}", target.display()), a.span))?;
    if !canon.is_dir() {
        return Err(ShellError::rt(format!("not a directory: {}", target.display()), a.span));
    }
    ev.g.env.borrow_mut().insert("PWD".into(), canon.to_string_lossy().into_owned());
    *ev.g.cwd.borrow_mut() = canon;
    Ok(PipelineData::Empty)
}

fn pwd(ev: &mut Eval, a: &CallArgs, _i: PipelineData) -> R {
    ok(Value::string(ev.g.cwd.borrow().to_string_lossy().into_owned(), a.span))
}

fn env(ev: &mut Eval, a: &CallArgs, _i: PipelineData) -> R {
    let rows =
        ev.g.env
            .borrow()
            .iter()
            .map(|(k, v)| {
                Value::record(
                    vec![
                        ("name".into(), Value::string(k.clone(), a.span)),
                        ("value".into(), Value::string(v.clone(), a.span)),
                    ],
                    a.span,
                )
            })
            .collect();
    ok(Value::list(rows, a.span))
}

fn help(ev: &mut Eval, a: &CallArgs, _i: PipelineData) -> R {
    if let Some(c) = a.opt(0) {
        let mut name = c.to_text();
        for extra in &a.positional[1..] {
            name.push(' ');
            name.push_str(&extra.to_text());
        }
        let defs = ev.g.defs.borrow();
        let sig = ev.g.builtins.get(&name).map(|b| b.sig.clone()).or_else(|| defs.get(&name).map(|d| d.sig.clone()));
        let Some(sig) = sig else {
            return Err(ShellError::rt(format!("no help for unknown command '{name}'"), c.span));
        };
        let mut s = format!("{}\n\nUsage:\n  {}\n", sig.desc, sig.usage());
        if !sig.io.is_empty() {
            s.push_str("\nInput/output types:\n");
            for (i, o) in &sig.io {
                s.push_str(&format!("  {i} -> {o}\n"));
            }
        }
        return ok(Value::string(s.trim_end().to_string(), a.span));
    }
    let mut names: Vec<(&String, &Builtin)> = ev.g.builtins.iter().collect();
    names.sort_by_key(|(n, _)| *n);
    let rows = names
        .into_iter()
        .map(|(n, b)| {
            Value::record(
                vec![
                    ("name".into(), Value::string(n.clone(), a.span)),
                    ("description".into(), Value::string(b.sig.desc.clone(), a.span)),
                ],
                a.span,
            )
        })
        .collect();
    ok(Value::list(rows, a.span))
}

fn table(ev: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let w = match a.flag("width") {
        Some(v) => v.as_int()? as usize,
        None => ev.g.width.get(),
    };
    let v = input.into_value(a.span)?;
    ok(Value::string(crate::table::render(&v, w).trim_end().to_string(), a.span))
}

fn enumerate(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let span = a.span;
    let it = input.into_iter_values(span).enumerate().map(move |(i, x)| {
        x.map(|v| Value::record(vec![("index".into(), Value::int(i as i64, span)), ("item".into(), v)], span))
    });
    Ok(PipelineData::stream(it, span))
}

fn wrap(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let name = a.req(0)?.to_text();
    let span = a.span;
    match input {
        PipelineData::Value(v) if !matches!(v.v, Val::List(_) | Val::Range(_)) => {
            ok(Value::record(vec![(name, v)], span))
        }
        other => {
            let it = other.into_iter_values(span).map(move |x| x.map(|v| Value::record(vec![(name.clone(), v)], span)));
            Ok(PipelineData::stream(it, span))
        }
    }
}

fn append(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let v = a.req(0)?.clone();
    let extra: Vec<Value> = match v.v {
        Val::List(l) => l,
        _ => vec![v],
    };
    let mut items = input_list(input, a.span)?;
    if current_cmd() == "prepend" {
        let mut e = extra;
        e.extend(items);
        items = e;
    } else {
        items.extend(extra);
    }
    ok(Value::list(items, a.span))
}

fn compact(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let it = input.into_iter_values(a.span).filter(|x| !matches!(x, Ok(v) if matches!(v.v, Val::Nothing)));
    Ok(PipelineData::stream(it, a.span))
}

fn into(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let cmd = current_cmd();
    let v = input.into_value(a.span)?;
    let sp = v.span;
    let bad =
        |v: &Value| ShellError::rt(format!("cannot convert {} '{}' with {cmd}", v.type_name(), v.to_display()), v.span);
    let out = match cmd.as_str() {
        "into int" => match &v.v {
            Val::Int(_) => v.clone(),
            Val::Float(f) => Value::int(*f as i64, sp),
            Val::Bool(b) => Value::int(*b as i64, sp),
            Val::Filesize(b) | Val::Duration(b) => Value::int(*b, sp),
            Val::String(s) => Value::int(s.trim().parse().map_err(|_| bad(&v))?, sp),
            _ => return Err(bad(&v)),
        },
        "into float" => match &v.v {
            Val::Int(i) => Value::float(*i as f64, sp),
            Val::Float(_) => v.clone(),
            Val::String(s) => Value::float(s.trim().parse().map_err(|_| bad(&v))?, sp),
            _ => return Err(bad(&v)),
        },
        _ => Value::string(v.to_text(), sp),
    };
    ok(out)
}

fn is_empty(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let v = input.into_value(a.span)?;
    let e = match &v.v {
        Val::Nothing => true,
        Val::String(s) => s.is_empty(),
        Val::List(l) => l.is_empty(),
        Val::Record(r) => r.is_empty(),
        _ => false,
    };
    ok(Value::bool(e, a.span))
}

fn ignore(_: &mut Eval, _a: &CallArgs, input: PipelineData) -> R {
    crate::eval::drain(input)?;
    Ok(PipelineData::Empty)
}

fn columns(_: &mut Eval, a: &CallArgs, input: PipelineData) -> R {
    let v = input.into_value(a.span)?;
    let mut cols: Vec<String> = Vec::new();
    let rows = match &v.v {
        Val::List(l) => l.clone(),
        _ => vec![v.clone()],
    };
    for r in rows {
        match &r.v {
            Val::Record(rec) => {
                for (k, _) in rec {
                    if !cols.contains(k) {
                        cols.push(k.clone());
                    }
                }
            }
            _ => {
                return Err(ShellError::rt(
                    format!("columns expects a record or table, found {}", r.type_name()),
                    r.span,
                ));
            }
        }
    }
    ok(Value::list(cols.into_iter().map(|c| Value::string(c, a.span)).collect(), a.span))
}

fn error_make(_: &mut Eval, a: &CallArgs, _i: PipelineData) -> R {
    Err(ShellError::rt(a.req(0)?.to_text(), a.span))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matching() {
        assert!(wildcard("*.json", "a.json"));
        assert!(!wildcard("*.json", "a.csv"));
        assert!(wildcard("a?c*", "abcdef"));
        assert!(wildcard("*", ""));
        assert!(!wildcard("a*b", "acd"));
    }

    #[test]
    fn at_least_forty_builtins() {
        assert!(builtins().len() >= 40, "{}", builtins().len());
    }
}
