//! Tokenizer. Shell syntax is whitespace sensitive, so tokens remember whether
//! whitespace preceded them.

use crate::span::{ErrorKind, ShellError, Span};
use crate::value::PathMember;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Int(i64),
    Float(f64),
    Filesize(i64),
    Duration(i64),
    Str(String),
    /// `$"..."`: alternating literal/expression parts. Expressions carry their
    /// source text and absolute offset.
    Interp(Vec<InterpPart>),
    Var(String, Vec<PathMember>),
    PathTail(Vec<PathMember>),
    Word(String),
    Flag(String),
    Op(&'static str),
    Pipe,
    Semi,
    Newline,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    DotDot,
    DotDotLt,
    Caret,
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InterpPart {
    Lit(String),
    Expr(String, usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

impl Tok {
    pub fn describe(&self) -> String {
        match self {
            Tok::Int(i) => format!("integer {i}"),
            Tok::Float(f) => format!("float {f}"),
            Tok::Filesize(_) => "filesize".into(),
            Tok::Duration(_) => "duration".into(),
            Tok::Str(_) | Tok::Interp(_) => "string".into(),
            Tok::Var(n, _) => format!("variable ${n}"),
            Tok::PathTail(_) => "cell path".into(),
            Tok::Word(w) => format!("'{w}'"),
            Tok::Flag(f) => format!("flag --{f}"),
            Tok::Op(o) => format!("'{o}'"),
            Tok::Pipe => "'|'".into(),
            Tok::Semi => "';'".into(),
            Tok::Newline => "newline".into(),
            Tok::LParen => "'('".into(),
            Tok::RParen => "')'".into(),
            Tok::LBracket => "'['".into(),
            Tok::RBracket => "']'".into(),
            Tok::LBrace => "'{'".into(),
            Tok::RBrace => "'}'".into(),
            Tok::Comma => "','".into(),
            Tok::Colon => "':'".into(),
            Tok::DotDot => "'..'".into(),
            Tok::DotDotLt => "'..<'".into(),
            Tok::Caret => "'^'".into(),
            Tok::Eof => "end of input".into(),
        }
    }
}

const OPS: [&str; 20] =
    ["==", "!=", "<=", ">=", "<", ">", "+", "-", "*", "/", "//", "**", "++", "=~", "!~", "=", "+=", "-=", "*=", "|"];

fn is_delim(c: char) -> bool {
    c.is_whitespace() || matches!(c, '|' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | '"' | '\'' | '`')
}

pub fn lex(src: &str) -> Result<Vec<Token>, ShellError> {
    lex_at(src, 0)
}

/// Lex `src`, whose first byte sits at absolute offset `base`.
pub fn lex_at(src: &str, base: usize) -> Result<Vec<Token>, ShellError> {
    let b = src.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0;
    let push = |toks: &mut Vec<Token>, tok: Tok, s: usize, e: usize| {
        toks.push(Token { tok, span: Span::new(base + s, base + e) });
    };
    while i < b.len() {
        let c = src[i..].chars().next().unwrap();
        let start = i;
        match c {
            '\n' => {
                push(&mut toks, Tok::Newline, i, i + 1);
                i += 1;
            }
            c if c.is_whitespace() => i += c.len_utf8(),
            '#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            '|' => {
                push(&mut toks, Tok::Pipe, i, i + 1);
                i += 1;
            }
            ';' => {
                push(&mut toks, Tok::Semi, i, i + 1);
                i += 1;
            }
            '(' => {
                push(&mut toks, Tok::LParen, i, i + 1);
                i += 1;
            }
            ')' => {
                push(&mut toks, Tok::RParen, i, i + 1);
                i += 1;
                if i + 1 < b.len()
                    && b[i] == b'.'
                    && !is_delim(src[i + 1..].chars().next().unwrap())
                    && b[i + 1] != b'.'
                {
                    let s = i;
                    while i < b.len() && !is_delim(src[i..].chars().next().unwrap()) {
                        i += src[i..].chars().next().unwrap().len_utf8();
                    }
                    let path = parse_path(&src[s + 1..i], base + s + 1);
                    push(&mut toks, Tok::PathTail(path), s, i);
                }
            }
            '[' => {
                push(&mut toks, Tok::LBracket, i, i + 1);
                i += 1;
            }
            ']' => {
                push(&mut toks, Tok::RBracket, i, i + 1);
                i += 1;
            }
            '{' => {
                push(&mut toks, Tok::LBrace, i, i + 1);
                i += 1;
            }
            '}' => {
                push(&mut toks, Tok::RBrace, i, i + 1);
                i += 1;
            }
            ',' => {
                push(&mut toks, Tok::Comma, i, i + 1);
                i += 1;
            }
            '"' => {
                let (s, end) = lex_dq(src, i + 1, base)?;
                push(&mut toks, Tok::Str(s), start, end);
                i = end;
            }
            '\'' | '`' => {
                let close = src[i + 1..].find(c).ok_or_else(|| {
                    ShellError::new(
                        ErrorKind::Parse,
                        "unterminated string",
                        Span::new(base + i, base + src.len()),
                        "string starts here",
                    )
                })?;
                let s = src[i + 1..i + 1 + close].to_string();
                i = i + 2 + close;
                let tok = if c == '`' { Tok::Word(s) } else { Tok::Str(s) };
                push(&mut toks, tok, start, i);
            }
            '$' if src[i + 1..].starts_with('"') => {
                let (parts, end) = lex_interp(src, i + 2, base)?;
                push(&mut toks, Tok::Interp(parts), start, end);
                i = end;
            }
            '^' if i + 1 < b.len() && !is_delim(src[i + 1..].chars().next().unwrap()) => {
                push(&mut toks, Tok::Caret, i, i + 1);
                i += 1;
            }
            _ => {
                while i < b.len() && !is_delim(src[i..].chars().next().unwrap()) {
                    i += src[i..].chars().next().unwrap().len_utf8();
                }
                classify_chunk(&src[start..i], base + start, &mut toks)?;
            }
        }
    }
    toks.push(Token { tok: Tok::Eof, span: Span::new(base + src.len(), base + src.len()) });
    Ok(toks)
}

fn lex_dq(src: &str, mut i: usize, base: usize) -> Result<(String, usize), ShellError> {
    let mut out = String::new();
    let start = i - 1;
    let mut chars = src[i..].chars();
    loop {
        let Some(c) = chars.next() else {
            return Err(ShellError::new(
                ErrorKind::Parse,
                "unterminated string",
                Span::new(base + start, base + src.len()),
                "string starts here",
            ));
        };
        i += c.len_utf8();
        match c {
            '"' => return Ok((out, i)),
            '\\' => {
                let e = chars.next().unwrap_or('\\');
                i += e.len_utf8();
                out.push(match e {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    '0' => '\0',
                    other => other,
                });
            }
            c => out.push(c),
        }
    }
}

fn lex_interp(src: &str, mut i: usize, base: usize) -> Result<(Vec<InterpPart>, usize), ShellError> {
    let start = i - 2;
    let mut parts = Vec::new();
    let mut lit = String::new();
    let b = src.as_bytes();
    while i < b.len() {
        let c = src[i..].chars().next().unwrap();
        match c {
            '"' => {
                if !lit.is_empty() {
                    parts.push(InterpPart::Lit(lit));
                }
                return Ok((parts, i + 1));
            }
            '\\' if i + 1 < b.len() => {
                let e = src[i + 1..].chars().next().unwrap();
                lit.push(match e {
                    'n' => '\n',
                    't' => '\t',
                    other => other,
                });
                i += 1 + e.len_utf8();
            }
            '(' => {
                if !lit.is_empty() {
                    parts.push(InterpPart::Lit(std::mem::take(&mut lit)));
                }
                let mut depth = 1;
                let s = i + 1;
                i += 1;
                while i < b.len() && depth > 0 {
                    match b[i] {
                        b'(' => depth += 1,
                        b')' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                if depth > 0 {
                    break;
                }
                parts.push(InterpPart::Expr(src[s..i - 1].to_string(), base + s));
            }
            c => {
                lit.push(c);
                i += c.len_utf8();
            }
        }
    }
    Err(ShellError::new(
        ErrorKind::Parse,
        "unterminated string interpolation",
        Span::new(base + start, base + src.len()),
        "interpolation starts here",
    ))
}

fn parse_path(s: &str, base: usize) -> Vec<PathMember> {
    let mut out = Vec::new();
    let mut off = base;
    for part in s.split('.') {
        out.push(PathMember::parse(part, Span::new(off, off + part.len())));
        off += part.len() + 1;
    }
    out
}

fn number(s: &str) -> Option<Tok> {
    let clean: String = s.chars().filter(|c| *c != '_').collect();
    if clean.is_empty()
        || !(clean.starts_with(|c: char| c.is_ascii_digit()) || clean.starts_with('-') && clean.len() > 1)
    {
        return None;
    }
    if let Ok(i) = clean.parse::<i64>() {
        return Some(Tok::Int(i));
    }
    if let Some(h) = clean.strip_prefix("0x")
        && let Ok(i) = i64::from_str_radix(h, 16)
    {
        return Some(Tok::Int(i));
    }
    if clean.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == 'e')
        && let Ok(f) = clean.parse::<f64>()
    {
        return Some(Tok::Float(f));
    }
    // unit suffix
    let pos = clean.find(|c: char| c.is_ascii_alphabetic())?;
    let (num, unit) = clean.split_at(pos);
    let n: f64 = num.parse().ok()?;
    let size = |m: f64| Some(Tok::Filesize((n * m) as i64));
    let dur = |m: f64| Some(Tok::Duration((n * m) as i64));
    match unit.to_ascii_lowercase().as_str() {
        "b" => size(1.0),
        "kb" => size(1000.0),
        "mb" => size(1e6),
        "gb" => size(1e9),
        "tb" => size(1e12),
        "kib" => size(1024.0),
        "mib" => size(1048576.0),
        "gib" => size(1073741824.0),
        "ns" => dur(1.0),
        "us" => dur(1e3),
        "ms" => dur(1e6),
        "sec" => dur(1e9),
        "min" => dur(6e10),
        "hr" => dur(3.6e12),
        "day" => dur(8.64e13),
        "wk" => dur(6.048e14),
        _ => None,
    }
}

fn classify_chunk(s: &str, base: usize, toks: &mut Vec<Token>) -> Result<(), ShellError> {
    let span = Span::new(base, base + s.len());
    let mut push = |tok: Tok, a: usize, b: usize| toks.push(Token { tok, span: Span::new(base + a, base + b) });
    if let Some(op) = OPS.iter().find(|o| **o == s) {
        push(Tok::Op(op), 0, s.len());
        return Ok(());
    }
    // trailing colon: `name:` in records/signatures
    if s.len() > 1 && s.ends_with(':') && !s.ends_with("::") {
        classify_chunk(&s[..s.len() - 1], base, toks)?;
        toks.push(Token { tok: Tok::Colon, span: Span::new(span.end - 1, span.end) });
        return Ok(());
    }
    if s == ":" {
        push(Tok::Colon, 0, 1);
        return Ok(());
    }
    // ranges: 1..10, 1..<10, 1.., $a..$b
    if let Some(p) = s.find("..")
        && p > 0
    {
        let (l, r) = (&s[..p], &s[p + 2..]);
        let side_ok = |x: &str| x.is_empty() || number(x).is_some() || x.starts_with('$');
        let (r_body, lt) = match r.strip_prefix('<') {
            Some(t) => (t, true),
            None => (r, false),
        };
        if (number(l).is_some() || l.starts_with('$')) && side_ok(r_body) && !r_body.contains("..") {
            classify_chunk(l, base, toks)?;
            let op_len = if lt { 3 } else { 2 };
            toks.push(Token {
                tok: if lt { Tok::DotDotLt } else { Tok::DotDot },
                span: Span::new(base + p, base + p + op_len),
            });
            if !r_body.is_empty() {
                classify_chunk(r_body, base + p + op_len, toks)?;
            }
            return Ok(());
        }
    }
    // `-$x`, `-(...)`: unary minus glued to an operand
    if let Some(rest) = s.strip_prefix('-')
        && rest.starts_with('$')
    {
        push(Tok::Op("-"), 0, 1);
        return classify_chunk(rest, base + 1, toks);
    }
    if let Some(rest) = s.strip_prefix('$') {
        let (name, path) = match rest.find('.') {
            Some(d) => (&rest[..d], parse_path(&rest[d + 1..], base + 2 + d)),
            None => (rest, vec![]),
        };
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            return Err(ShellError::new(ErrorKind::Parse, "invalid variable name", span, "here"));
        }
        push(Tok::Var(name.to_string(), path), 0, s.len());
        return Ok(());
    }
    if let Some(t) = number(s) {
        push(t, 0, s.len());
        return Ok(());
    }
    if let Some(f) = s.strip_prefix("--")
        && f.starts_with(|c: char| c.is_alphabetic())
    {
        push(Tok::Flag(f.to_string()), 0, s.len());
        return Ok(());
    }
    if let Some(f) = s.strip_prefix('-')
        && f.len() == 1
        && f.starts_with(|c: char| c.is_alphabetic())
    {
        push(Tok::Flag(format!("-{f}")), 0, s.len());
        return Ok(());
    }
    push(Tok::Word(s.to_string()), 0, s.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> Vec<Tok> {
        lex(s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn lexes_basic_pipeline() {
        assert_eq!(
            kinds("ls | where size > 10kb"),
            vec![
                Tok::Word("ls".into()),
                Tok::Pipe,
                Tok::Word("where".into()),
                Tok::Word("size".into()),
                Tok::Op(">"),
                Tok::Filesize(10_000),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn lexes_ranges_and_numbers() {
        assert_eq!(kinds("1..1_000"), vec![Tok::Int(1), Tok::DotDot, Tok::Int(1000), Tok::Eof]);
        assert_eq!(kinds("0..<5"), vec![Tok::Int(0), Tok::DotDotLt, Tok::Int(5), Tok::Eof]);
        assert_eq!(kinds("3.5 -2 5sec"), vec![Tok::Float(3.5), Tok::Int(-2), Tok::Duration(5_000_000_000), Tok::Eof]);
        assert_eq!(kinds("cd .."), vec![Tok::Word("cd".into()), Tok::Word("..".into()), Tok::Eof]);
    }

    #[test]
    fn lexes_vars_with_paths_and_spans() {
        let t = lex("$x.a.0").unwrap();
        match &t[0].tok {
            Tok::Var(n, p) => {
                assert_eq!(n, "x");
                assert_eq!(p.len(), 2);
                assert_eq!(p[1].index, Some(0));
                assert_eq!(p[1].span, Span::new(5, 6));
            }
            _ => panic!(),
        }
        assert_eq!(t[0].span, Span::new(0, 6));
    }

    #[test]
    fn lexes_strings_flags_interp() {
        assert_eq!(
            kinds(r#"echo "a\nb" 'r\n' --flag -f"#),
            vec![
                Tok::Word("echo".into()),
                Tok::Str("a\nb".into()),
                Tok::Str("r\\n".into()),
                Tok::Flag("flag".into()),
                Tok::Flag("-f".into()),
                Tok::Eof
            ]
        );
        let t = lex(r#"$"hi ($x + 1)!""#).unwrap();
        assert_eq!(
            t[0].tok,
            Tok::Interp(vec![
                InterpPart::Lit("hi ".into()),
                InterpPart::Expr("$x + 1".into(), 6),
                InterpPart::Lit("!".into())
            ])
        );
    }

    #[test]
    fn unterminated_string_has_span() {
        let e = lex(r#"echo "abc"#).unwrap_err();
        assert_eq!(e.span, Span::new(5, 9));
    }

    #[test]
    fn record_keys_and_path_tail() {
        assert_eq!(
            kinds("{a: 1}"),
            vec![Tok::LBrace, Tok::Word("a".into()), Tok::Colon, Tok::Int(1), Tok::RBrace, Tok::Eof]
        );
        let k = kinds("(ls).name");
        assert!(matches!(&k[3], Tok::PathTail(p) if p[0].name == "name"));
    }
}
