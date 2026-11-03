//! Table or JSON output.

use std::io::Write;

use serde_json::Value;

/// A value of a result row. Numbers stay numbers in JSON; names that look like numbers (a year
/// `2026`) stay text.
#[derive(Debug, Clone, PartialEq)]
pub enum V {
    Text(String),
    Num(f64),
    Empty,
}

impl V {
    pub fn text(s: impl Into<String>) -> V {
        V::Text(s.into())
    }

    pub fn opt(v: Option<f64>) -> V {
        v.map_or(V::Empty, V::Num)
    }

    fn show(&self) -> String {
        match self {
            V::Text(s) => s.clone(),
            V::Num(n) => n.to_string(),
            V::Empty => String::new(),
        }
    }

    fn json(&self) -> Value {
        match self {
            V::Text(s) => Value::String(s.clone()),
            V::Num(n) => serde_json::Number::from_f64(*n).map_or(Value::Null, Value::Number),
            V::Empty => Value::Null,
        }
    }
}

/// Where results and messages go; tests collect them in memory.
pub struct Output<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    pub json: bool,
}

impl Output<'_> {
    pub fn line(&mut self, text: impl AsRef<str>) {
        writeln!(self.out, "{}", text.as_ref()).ok();
    }

    pub fn note(&mut self, text: impl AsRef<str>) {
        writeln!(self.err, "{}", text.as_ref()).ok();
    }

    /// A table: aligned columns, or an array of objects with `--json`.
    pub fn table(&mut self, headers: &[&str], rows: &[Vec<V>]) {
        if self.json {
            let items: Vec<Value> = rows
                .iter()
                .map(|r| {
                    Value::Object(
                        headers
                            .iter()
                            .zip(r)
                            .map(|(h, v)| ((*h).to_string(), v.json()))
                            .collect(),
                    )
                })
                .collect();
            self.line(serde_json::to_string(&items).unwrap_or_default());
            return;
        }
        let shown: Vec<Vec<String>> = rows
            .iter()
            .map(|r| r.iter().map(V::show).collect())
            .collect();
        let widths: Vec<usize> = (0..headers.len())
            .map(|c| {
                shown
                    .iter()
                    .map(|r| r[c].chars().count())
                    .chain([headers[c].chars().count()])
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let fmt = |cells: &[&str]| -> String {
            let n = cells.len();
            cells
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    if i + 1 == n {
                        (*c).to_string()
                    } else {
                        format!("{c:<w$}", w = widths[i])
                    }
                })
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_string()
        };
        self.line(fmt(headers));
        for r in &shown {
            self.line(fmt(&r.iter().map(String::as_str).collect::<Vec<_>>()));
        }
    }

    /// One object (`--json`) or aligned `key  value` lines.
    pub fn record(&mut self, fields: &[(&str, V)]) {
        if self.json {
            let v: serde_json::Map<String, Value> = fields
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.json()))
                .collect();
            self.line(serde_json::to_string(&Value::Object(v)).unwrap_or_default());
        } else {
            let w = fields.iter().map(|f| f.0.len()).max().unwrap_or(0);
            for (k, v) in fields {
                self.line(format!("{k:<w$}  {}", v.show()));
            }
        }
    }
}
