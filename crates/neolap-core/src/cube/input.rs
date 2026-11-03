//! Parsing of cell input (splash syntax).

use crate::{Error, Result};

/// A write request for a cell. Aggregates are written by splashing onto their base cells.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// Empty input: deletes all base cells of the area.
    Clear,
    /// `500` / `=-500`: distributes the target proportionally to the existing values.
    Set(f64),
    /// `#500`: distributes equally over all base combinations of the area.
    Equal(f64),
    /// `+50` / `-50`: adds the difference proportionally.
    Delta(f64),
    /// `+5%` / `-3%`: scales all base values of the area.
    Percent(f64),
    /// `@Actual,2026:500`: distributes proportionally to the structure of another area.
    Reference { members: Vec<String>, target: f64 },
}

fn number(s: &str) -> Result<f64> {
    let s: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '\'')
        .collect();
    let (comma, dot) = (s.rfind(','), s.rfind('.'));
    let normalized = match (comma, dot) {
        (Some(c), Some(d)) if c > d => s.replace('.', "").replace(',', "."),
        (Some(_), Some(_)) => s.replace(',', ""),
        (Some(_), None) => s.replace(',', "."),
        _ => s.clone(),
    };
    match normalized.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(Error::Parse(format!("'{s}' is not a number"))),
    }
}

impl Input {
    /// Parses the text of a cell input. A leading `+`/`-` means a difference, so a negative
    /// absolute value is written as `=-50`.
    pub fn parse(text: &str) -> Result<Input> {
        let t = text.trim();
        if t.is_empty() {
            return Ok(Input::Clear);
        }
        if let Some(rest) = t.strip_prefix('@') {
            let (refs, target) = rest.rsplit_once(':').ok_or_else(|| {
                Error::Parse("reference input needs the form '@member,member:value'".into())
            })?;
            let members: Vec<String> = refs
                .split(',')
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty())
                .collect();
            if members.is_empty() {
                return Err(Error::Parse("reference input names no member".into()));
            }
            return Ok(Input::Reference {
                members,
                target: number(target)?,
            });
        }
        if let Some(rest) = t.strip_prefix('#') {
            return Ok(Input::Equal(number(rest)?));
        }
        if let Some(rest) = t.strip_prefix('=') {
            return Ok(Input::Set(number(rest)?));
        }
        if let Some(rest) = t.strip_suffix('%') {
            return Ok(Input::Percent(number(rest)?));
        }
        if t.starts_with(['+', '-']) {
            return Ok(Input::Delta(number(t)?));
        }
        Ok(Input::Set(number(t)?))
    }
}
