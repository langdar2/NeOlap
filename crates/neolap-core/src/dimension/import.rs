//! Dimension import from parent-child lists and level columns (CSV).

use std::collections::HashSet;
use std::io::Read;

use super::{AttrValue, DimensionEditor};
use crate::{Error, MemberId, Result};

/// How the file is merged into the existing dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImportMode {
    /// Only add members, edges and attributes.
    #[default]
    AddOnly,
    /// Additionally remove members that do not occur in the file.
    Sync,
}

/// Log of an import.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    pub rows: usize,
    pub members_added: usize,
    /// Names removed in [`ImportMode::Sync`].
    pub removed: Vec<String>,
}

fn reader<R: Read>(input: R, delimiter: u8) -> csv::Reader<R> {
    csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(true)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(input)
}

fn csv_err(e: csv::Error) -> Error {
    Error::Parse(e.to_string())
}

fn attr_value(s: &str) -> AttrValue {
    s.parse::<f64>()
        .map_or_else(|_| AttrValue::Text(s.to_string()), AttrValue::Number)
}

fn finish(
    ed: &mut DimensionEditor,
    mode: ImportMode,
    before: usize,
    seen: HashSet<MemberId>,
    mut report: ImportReport,
    existing: Vec<MemberId>,
) -> Result<ImportReport> {
    report.members_added = ed.len() - before;
    if mode == ImportMode::Sync {
        // `existing` holds the live ids from before the import; the plan's "Löschvorschau" is `removed`.
        for id in existing.into_iter().filter(|id| !seen.contains(id)) {
            report.removed.push(ed.name_of(id));
            ed.remove(id)?;
        }
    }
    Ok(report)
}

/// Imports `child;parent;weight[;attribute…]` rows. The header names the columns: `child`,
/// `parent` (empty = root), `weight` (optional, default 1), `alias` (optional, several separated by `|`);
/// all other columns are attributes.
pub fn import_parent_child<R: Read>(
    ed: &mut DimensionEditor,
    input: R,
    delimiter: u8,
    mode: ImportMode,
) -> Result<ImportReport> {
    let mut rdr = reader(input, delimiter);
    let headers = rdr.headers().map_err(csv_err)?.clone();
    let col = |n: &str| headers.iter().position(|h| h.eq_ignore_ascii_case(n));
    let c_child = col("child").ok_or_else(|| Error::Parse("column 'child' missing".into()))?;
    let c_parent = col("parent");
    let c_weight = col("weight");
    let c_alias = col("alias");

    let existing = ed.live_ids();
    let before = ed.len();
    let mut seen = HashSet::new();
    let mut report = ImportReport::default();
    for rec in rdr.records() {
        let rec = rec.map_err(csv_err)?;
        report.rows += 1;
        let line = report.rows + 1;
        let child_name = rec.get(c_child).unwrap_or("");
        if child_name.is_empty() {
            return Err(Error::Parse(format!("line {line}: empty child")));
        }
        let child = ed.get_or_add(child_name)?;
        seen.insert(child);
        if let Some(parent_name) = c_parent.and_then(|c| rec.get(c)).filter(|p| !p.is_empty()) {
            let parent = ed.get_or_add(parent_name)?;
            seen.insert(parent);
            let weight = match c_weight.and_then(|c| rec.get(c)).filter(|w| !w.is_empty()) {
                Some(w) => w
                    .replace(',', ".")
                    .parse::<f32>()
                    .map_err(|_| Error::Parse(format!("line {line}: invalid weight '{w}'")))?,
                None => 1.0,
            };
            ed.add_edge(parent, child, weight)?;
        }
        for alias in c_alias
            .and_then(|c| rec.get(c))
            .unwrap_or("")
            .split('|')
            .filter(|a| !a.is_empty())
        {
            ed.set_alias(child, alias)?;
        }
        for (i, h) in headers.iter().enumerate() {
            if i == c_child || Some(i) == c_parent || Some(i) == c_weight || Some(i) == c_alias {
                continue;
            }
            if let Some(v) = rec.get(i).filter(|v| !v.is_empty()) {
                ed.set_attribute(h, child, attr_value(v))?;
            }
        }
    }
    finish(ed, mode, before, seen, report, existing)
}

/// Imports level columns (`L1;L2;…;Ln`, topmost first). Each row is a path from a root down to a
/// base member; trailing empty cells end the path. Weights are 1.
pub fn import_levels<R: Read>(
    ed: &mut DimensionEditor,
    input: R,
    delimiter: u8,
    mode: ImportMode,
) -> Result<ImportReport> {
    let mut rdr = reader(input, delimiter);
    rdr.headers().map_err(csv_err)?;
    let existing = ed.live_ids();
    let before = ed.len();
    let mut seen = HashSet::new();
    let mut report = ImportReport::default();
    for rec in rdr.records() {
        let rec = rec.map_err(csv_err)?;
        report.rows += 1;
        let mut parent: Option<MemberId> = None;
        for cell in rec.iter().take_while(|c| !c.is_empty()) {
            let id = ed.get_or_add(cell)?;
            seen.insert(id);
            if let Some(p) = parent {
                ed.add_edge(p, id, 1.0)?;
            }
            parent = Some(id);
        }
    }
    finish(ed, mode, before, seen, report, existing)
}
