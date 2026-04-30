//! Source spans and error reporting.

use std::fmt;

/// Byte range into the source text.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start, end }
    }
    pub fn unknown() -> Span {
        Span { start: 0, end: 0 }
    }
    pub fn merge(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}..{}", self.start, self.end)
    }
}

/// Any error reported to the user: parse, type or runtime.
#[derive(Clone, Debug, PartialEq)]
pub struct ShellError {
    pub kind: ErrorKind,
    pub msg: String,
    pub span: Span,
    pub label: String,
    pub help: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Parse,
    Type,
    Runtime,
}

impl ShellError {
    pub fn new(kind: ErrorKind, msg: impl Into<String>, span: Span, label: impl Into<String>) -> Self {
        ShellError { kind, msg: msg.into(), span, label: label.into(), help: None }
    }
    pub fn rt(msg: impl Into<String>, span: Span) -> Self {
        let m = msg.into();
        ShellError::new(ErrorKind::Runtime, m.clone(), span, m)
    }
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Render the error with the offending source line and a caret underline.
    pub fn render(&self, source: &str, file: &str) -> String {
        let kind = match self.kind {
            ErrorKind::Parse => "parse error",
            ErrorKind::Type => "type error",
            ErrorKind::Runtime => "error",
        };
        let mut out = format!("{kind}: {}\n", self.msg);
        if self.span.end > 0 || self.span.start > 0 {
            let start = self.span.start.min(source.len());
            let end = self.span.end.clamp(start, source.len());
            let line_start = source[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let line_end = source[start..].find('\n').map(|i| start + i).unwrap_or(source.len());
            let line_no = source[..start].matches('\n').count() + 1;
            let col = source[line_start..start].chars().count();
            let width = source[start..end.min(line_end).max(start)].chars().count().max(1);
            let gutter = line_no.to_string().len();
            out.push_str(&format!("{:gutter$}--> {file}:{line_no}:{}\n", "", col + 1));
            out.push_str(&format!("{:gutter$} |\n", ""));
            out.push_str(&format!("{line_no} | {}\n", &source[line_start..line_end]));
            out.push_str(&format!("{:gutter$} | {}{} {}\n", "", " ".repeat(col), "^".repeat(width), self.label));
        }
        if let Some(h) = &self.help {
            out.push_str(&format!("  help: {h}\n"));
        }
        out
    }
}

impl fmt::Display for ShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl std::error::Error for ShellError {}
