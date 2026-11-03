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

/// A number as people type it. Both conventions work (`1.234,5` and `1,234.5`), but the structure
/// is checked: thousands groups have exactly three digits, there is at most one decimal part and
/// it comes last. Anything else (`1.5,5`, `1,,5`, `1.23.456`) is refused rather than turned into
/// some other number, because this text becomes a value in the database.
///
/// The reading is German, as in a German spreadsheet: the comma is the decimal sign (`1,234` is
/// 1.234), and a lone point followed by exactly three digits groups thousands (`2.000` is two
/// thousand, not 2). A point after anything else (`1.5`, `0.25`, `0.500`, `1234.567`) is a decimal point.
/// Pasting thousands from a spreadsheet must never turn into a fraction without a word. The price:
/// "1.000" cannot be written as one point zero zero zero. Several of the same separator can only
/// be thousands groups (`1.234.567`); with both kinds the last one is the decimal sign.
fn number(s: &str) -> Result<f64> {
    let s: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '\'')
        .collect();
    let bad = || Error::Parse(format!("'{s}' is not a number"));
    let (sign, body) = match s.strip_prefix(['-', '+']) {
        Some(rest) => (if s.starts_with('-') { -1.0 } else { 1.0 }, rest),
        None => (1.0, s.as_str()),
    };
    if body.is_empty()
        || !body
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
    {
        return Err(bad());
    }
    let groups_ok = |int_part: &str, sep: char| -> bool {
        // "1.234.567": a first group of 1-3 digits, then groups of exactly 3
        let mut parts = int_part.split(sep);
        let first = parts.next().unwrap_or("");
        (1..=3).contains(&first.len()) && parts.all(|p| p.len() == 3)
    };
    let digits = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit());
    let count = |c: char| body.matches(c).count();
    let (commas, dots) = (count(','), count('.'));
    let normalized = match (commas, dots) {
        (0, 0) if digits(body) => body.to_string(),
        // one separator, once: a decimal sign (".5" and "5,5" are fine, "5." and "." are not) —
        // except a point with exactly three digits after it, which groups thousands ("2.000")
        (1, 0) | (0, 1) => {
            let sep = if commas == 1 { ',' } else { '.' };
            let (int_part, frac) = body.split_once(sep).ok_or_else(bad)?;
            if !digits(frac) || !(int_part.is_empty() || digits(int_part)) {
                return Err(bad());
            }
            // a first group has one to three digits and no leading zero: "0.500" and "1234.567"
            // are decimals, "2.000" and "12.345" are thousands
            let first_group = (1..=3).contains(&int_part.len()) && !int_part.starts_with('0');
            if sep == '.' && frac.len() == 3 && first_group {
                format!("{int_part}{frac}")
            } else {
                format!(
                    "{}.{frac}",
                    if int_part.is_empty() { "0" } else { int_part }
                )
            }
        }
        // one separator kind, several times: thousands groups only
        (n, 0) | (0, n) if n > 1 => {
            let sep = if commas > 0 { ',' } else { '.' };
            if !groups_ok(body, sep) {
                return Err(bad());
            }
            body.replace(sep, "")
        }
        // both kinds: the last one is the decimal point, the other one groups thousands before it
        _ => {
            let (last_comma, last_dot) = (
                body.rfind(',').ok_or_else(bad)?,
                body.rfind('.').ok_or_else(bad)?,
            );
            let (dec_at, dec, group) = if last_comma > last_dot {
                (last_comma, ',', '.')
            } else {
                (last_dot, '.', ',')
            };
            let (int_part, frac) = (&body[..dec_at], &body[dec_at + 1..]);
            // the decimal point must occur once, and the grouping character only in the integer part
            if count(dec) != 1
                || frac.contains(group)
                || !digits(frac)
                || !groups_ok(int_part, group)
            {
                return Err(bad());
            }
            format!("{}.{frac}", int_part.replace(group, ""))
        }
    };
    match normalized.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(sign * v),
        _ => Err(bad()),
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
