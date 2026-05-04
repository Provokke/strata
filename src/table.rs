//! Table rendering with column fitting and truncation to a terminal width.

use crate::value::{Val, Value};

fn cell_text(v: &Value) -> String {
    v.to_display().replace(['\n', '\r'], " ").replace('\t', " ")
}

fn is_numeric(v: &Value) -> bool {
    matches!(v.v, Val::Int(_) | Val::Float(_) | Val::Filesize(_))
}

fn width_of(s: &str) -> usize {
    s.chars().count()
}

fn truncate(s: &str, w: usize) -> String {
    if width_of(s) <= w {
        s.to_string()
    } else if w == 0 {
        String::new()
    } else {
        let mut t: String = s.chars().take(w - 1).collect();
        t.push('…');
        t
    }
}

struct Grid {
    header: Option<Vec<String>>,
    rows: Vec<Vec<(String, bool)>>, // (text, right-align)
}

/// Render any value for display. Structured values become tables.
pub fn render(v: &Value, width: usize) -> String {
    match &v.v {
        Val::List(items) if items.is_empty() => "╭────────────╮\n│ empty list │\n╰────────────╯\n".into(),
        Val::List(items) => {
            let mut cols: Vec<String> = Vec::new();
            let mut any_record = false;
            for it in items {
                if let Val::Record(r) = &it.v {
                    any_record = true;
                    for (k, _) in r {
                        if !cols.contains(k) {
                            cols.push(k.clone());
                        }
                    }
                }
            }
            if any_record {
                let mut header = vec!["#".to_string()];
                header.extend(cols.iter().cloned());
                let rows = items
                    .iter()
                    .enumerate()
                    .map(|(i, it)| {
                        let mut row = vec![(i.to_string(), true)];
                        for c in &cols {
                            match it.get_field(c) {
                                Some(x) => row.push((cell_text(x), is_numeric(x))),
                                None if matches!(it.v, Val::Record(_)) => row.push(("-".into(), false)),
                                None => row.push((String::new(), false)),
                            }
                        }
                        row
                    })
                    .collect();
                draw(Grid { header: Some(header), rows }, width)
            } else {
                let rows = items
                    .iter()
                    .enumerate()
                    .map(|(i, it)| vec![(i.to_string(), true), (cell_text(it), is_numeric(it))])
                    .collect();
                draw(Grid { header: None, rows }, width)
            }
        }
        Val::Range(_) => {
            let items: Vec<Value> = match &v.v {
                Val::Range(r) => r.iter().take(10_000).map(|i| Value::int(i, v.span)).collect(),
                _ => unreachable!(),
            };
            render(&Value::list(items, v.span), width)
        }
        Val::Record(r) if r.is_empty() => "╭──────────────╮\n│ empty record │\n╰──────────────╯\n".into(),
        Val::Record(r) => {
            let rows = r.iter().map(|(k, x)| vec![(k.clone(), false), (cell_text(x), is_numeric(x))]).collect();
            draw(Grid { header: None, rows }, width)
        }
        Val::Nothing => String::new(),
        _ => format!("{}\n", v.to_display()),
    }
}

fn draw(g: Grid, width: usize) -> String {
    let ncols = g.rows.first().map(|r| r.len()).or(g.header.as_ref().map(|h| h.len())).unwrap_or(0);
    let mut natural = vec![0usize; ncols];
    if let Some(h) = &g.header {
        for (i, c) in h.iter().enumerate() {
            natural[i] = natural[i].max(width_of(c));
        }
    }
    for r in &g.rows {
        for (i, (c, _)) in r.iter().enumerate() {
            natural[i] = natural[i].max(width_of(c));
        }
    }
    // Choose how many columns fit (each needs at least min(natural, 4) + 3).
    let width = width.max(10);
    let border = |n: usize| 3 * n + 1;
    let mut keep = 0;
    let mut min_total = 0;
    for (i, w) in natural.iter().enumerate() {
        let need = (*w).min(4);
        // reserve room for the trailing "…" column if more columns follow
        let extra = if i + 1 < ncols { 4 } else { 0 };
        if keep > 0 && min_total + need + border(keep + 1) + extra > width {
            break;
        }
        min_total += need;
        keep += 1;
    }
    let truncated_cols = keep < ncols;
    let mut widths: Vec<usize> = natural[..keep].to_vec();
    let total = |ws: &[usize]| {
        ws.iter().sum::<usize>() + border(ws.len() + usize::from(truncated_cols)) + usize::from(truncated_cols)
    };
    // Shrink the widest non-index column until it fits.
    while total(&widths) > width {
        let (idx, w) =
            widths.iter().enumerate().skip(1).max_by_key(|(_, w)| **w).map(|(i, w)| (i, *w)).unwrap_or((0, 0));
        if w <= 4 {
            break;
        }
        widths[idx] -= 1;
    }
    let mut cols = widths.clone();
    if truncated_cols {
        cols.push(1);
    }
    let line = |l: &str, m: &str, r: &str| {
        let segs: Vec<String> = cols.iter().map(|w| "─".repeat(w + 2)).collect();
        format!("{l}{}{r}\n", segs.join(m))
    };
    let row_line = |cells: Vec<(String, bool)>| {
        let mut s = String::from("│");
        for (i, w) in cols.iter().enumerate() {
            let (text, right) = cells.get(i).cloned().unwrap_or_default();
            let t = truncate(&text, *w);
            let pad = w - width_of(&t);
            if right {
                s.push_str(&format!(" {}{} │", " ".repeat(pad), t));
            } else {
                s.push_str(&format!(" {}{} │", t, " ".repeat(pad)));
            }
        }
        s.push('\n');
        s
    };
    let mut out = line("╭", "┬", "╮");
    if let Some(h) = g.header {
        let mut cells: Vec<(String, bool)> = h.into_iter().take(keep).map(|c| (c, false)).collect();
        if truncated_cols {
            cells.push(("…".into(), false));
        }
        out.push_str(&row_line(cells));
        out.push_str(&line("├", "┼", "┤"));
    }
    for r in g.rows {
        let mut cells: Vec<(String, bool)> = r.into_iter().take(keep).collect();
        if truncated_cols {
            cells.push(("…".into(), false));
        }
        out.push_str(&row_line(cells));
    }
    out.push_str(&line("╰", "┴", "╯"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn s() -> Span {
        Span::unknown()
    }

    fn people() -> Value {
        let row = |n: &str, age: i64, city: &str, bio: &str| {
            Value::record(
                vec![
                    ("name".into(), Value::string(n, s())),
                    ("age".into(), Value::int(age, s())),
                    ("city".into(), Value::string(city, s())),
                    ("bio".into(), Value::string(bio, s())),
                ],
                s(),
            )
        };
        Value::list(
            vec![row("alice", 31, "Lisbon", "Writes compilers for fun and profit"), row("bob", 4, "Oslo", "Dog")],
            s(),
        )
    }

    #[test]
    fn snapshot_wide() {
        let expected = "\
╭───┬───────┬─────┬────────┬─────────────────────────────────────╮
│ # │ name  │ age │ city   │ bio                                 │
├───┼───────┼─────┼────────┼─────────────────────────────────────┤
│ 0 │ alice │  31 │ Lisbon │ Writes compilers for fun and profit │
│ 1 │ bob   │   4 │ Oslo   │ Dog                                 │
╰───┴───────┴─────┴────────┴─────────────────────────────────────╯
";
        assert_eq!(render(&people(), 120), expected);
    }

    #[test]
    fn snapshot_truncates_cells_at_50() {
        let out = render(&people(), 50);
        let expected = "\
╭───┬───────┬─────┬────────┬─────────────────────╮
│ # │ name  │ age │ city   │ bio                 │
├───┼───────┼─────┼────────┼─────────────────────┤
│ 0 │ alice │  31 │ Lisbon │ Writes compilers f… │
│ 1 │ bob   │   4 │ Oslo   │ Dog                 │
╰───┴───────┴─────┴────────┴─────────────────────╯
";
        assert_eq!(out, expected);
        for l in out.lines() {
            assert!(l.chars().count() <= 50, "{l}");
        }
    }

    #[test]
    fn snapshot_drops_columns_at_30() {
        let out = render(&people(), 30);
        let expected = "\
╭───┬───────┬─────┬──────┬───╮
│ # │ name  │ age │ city │ … │
├───┼───────┼─────┼──────┼───┤
│ 0 │ alice │  31 │ Lis… │ … │
│ 1 │ bob   │   4 │ Oslo │ … │
╰───┴───────┴─────┴──────┴───╯
";
        assert_eq!(out, expected);
    }

    #[test]
    fn every_width_fits() {
        for w in 20..140 {
            for l in render(&people(), w).lines() {
                assert!(l.chars().count() <= w, "width {w}: {l}");
            }
        }
    }

    #[test]
    fn snapshot_record_and_list() {
        let r = Value::record(vec![("a".into(), Value::int(1, s())), ("bb".into(), Value::string("x", s()))], s());
        assert_eq!(render(&r, 80), "╭────┬───╮\n│ a  │ 1 │\n│ bb │ x │\n╰────┴───╯\n");
        let l = Value::list(vec![Value::string("x", s()), Value::int(10, s())], s());
        assert_eq!(render(&l, 80), "╭───┬────╮\n│ 0 │ x  │\n│ 1 │ 10 │\n╰───┴────╯\n");
        assert_eq!(render(&Value::list(vec![], s()), 80), "╭────────────╮\n│ empty list │\n╰────────────╯\n");
    }

    #[test]
    fn missing_columns_marked() {
        let t = Value::list(
            vec![
                Value::record(vec![("a".into(), Value::int(1, s()))], s()),
                Value::record(vec![("b".into(), Value::int(2, s()))], s()),
            ],
            s(),
        );
        assert_eq!(
            render(&t, 80),
            "╭───┬───┬───╮\n│ # │ a │ b │\n├───┼───┼───┤\n│ 0 │ 1 │ - │\n│ 1 │ - │ 2 │\n╰───┴───┴───╯\n"
        );
    }
}
