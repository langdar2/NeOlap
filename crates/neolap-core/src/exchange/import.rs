//! Import run: map rows to cells, collect rejections, write as one operation.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::Serialize;

use super::spec::{ImportSpec, LoadMode, UnknownPolicy};
use super::table::{SourceKind, read_table};
use crate::cube::{BulkMode, BulkOutcome, BulkWrite, Cube, WriteContext};
use crate::dimension::{Dimension, DimensionKind};
use crate::{Error, MemberId, Result};

/// Creates unknown members: dimension index and `(name, parent)` pairs. Called before any cell is
/// written; the callback installs the edited dimension in the cube (and persists it).
pub type CreateMembers<'a> =
    &'a mut dyn FnMut(&mut Cube, usize, Vec<(String, String)>) -> Result<()>;

/// A row that was not loaded, with the reason.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RejectedRow {
    pub line: u64,
    pub reason: String,
}

/// Log of an import run.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ImportReport {
    pub spec: String,
    pub dry_run: bool,
    pub rows_read: u64,
    /// Cells derived from the rows (a wide row yields one cell per value column).
    pub cells_read: u64,
    pub cells_created: usize,
    pub cells_updated: usize,
    pub cells_deleted: usize,
    pub cells_unchanged: usize,
    pub rows_rejected: u64,
    pub rejected: Vec<RejectedRow>,
    /// Members that were (or, in a dry run, would be) created.
    pub members_created: Vec<String>,
    /// Journal operation of the write (`None` for dry runs and if nothing changed).
    pub op_id: Option<u64>,
    pub generation: u64,
    /// The import was stopped; nothing was written.
    pub failed: Option<String>,
}

/// The cells an import would write, ready to be applied later with [`apply_prepared`]. They hold
/// only member ids, so the expensive part (reading and resolving the file) can run while others
/// still read the cube, and only the short write needs the exclusive lock.
#[derive(Debug, Clone)]
pub struct PreparedImport {
    write: BulkWrite,
    /// Member slots per dimension at preparation time: a changed model makes the resolved member
    /// ids void. (Changed *cells* are caught by `bulk_write` through the caller's expected generation.)
    shape: Vec<usize>,
    pub report: ImportReport,
}

fn shape_of(cube: &Cube) -> Vec<usize> {
    cube.dimensions().iter().map(|d| d.len_slots()).collect()
}

/// Most rejected rows kept verbatim in the report.
const MAX_LISTED: usize = 200;

#[derive(Clone)]
enum Source {
    /// Fixed member for the whole file.
    Fixed(MemberId),
    /// Column index + whether aliases are allowed.
    Column(usize, bool),
    /// The dimension of the wide `[values]` columns.
    Values,
}

struct Plan {
    sources: Vec<Source>,
    /// (column index, member, column name) of the wide format.
    value_cols: Vec<(usize, MemberId, String)>,
    value_col: Option<usize>,
    decimal: char,
}

fn find_col(headers: &[String], name: &str, spec: &str) -> Result<usize> {
    headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            Error::Parse(format!(
                "import '{spec}': column '{name}' not found (available: {})",
                headers.join(", ")
            ))
        })
}

fn parse_number(text: &str, decimal: char) -> Option<f64> {
    let mut t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if decimal == ',' {
        t = t.replace('.', "").replace(',', ".");
    } else {
        t = t.replace(',', "");
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn dim_index(cube: &Cube, name: &str) -> Result<usize> {
    cube.dimensions()
        .iter()
        .position(|d| d.name().eq_ignore_ascii_case(name))
        .ok_or_else(|| Error::NotFound {
            kind: "dimension",
            name: format!("{name}' in cube '{}", cube.name()),
        })
}

fn make_plan(cube: &Cube, spec: &ImportSpec, headers: &[String]) -> Result<Plan> {
    let n = cube.dimensions().len();
    let mut sources: Vec<Option<Source>> = vec![None; n];
    let mut set = |d: usize, s: Source, what: &str| -> Result<()> {
        if sources[d].replace(s).is_some() {
            return Err(Error::Parse(format!(
                "import '{}': dimension '{}' is given twice ({what})",
                spec.name,
                cube.dimensions()[d].name()
            )));
        }
        Ok(())
    };
    for (dim, entry) in &spec.map {
        let d = dim_index(cube, dim)?;
        set(
            d,
            Source::Column(
                find_col(headers, entry.column(), &spec.name)?,
                entry.alias(),
            ),
            "map",
        )?;
    }
    for (dim, member) in &spec.fixed {
        let d = dim_index(cube, dim)?;
        let m = cube.dimensions()[d].member(member)?;
        set(d, Source::Fixed(m), "fixed")?;
    }
    let mut value_cols = Vec::new();
    if !spec.values.is_empty() {
        let d = match &spec.options.value_dimension {
            Some(name) => dim_index(cube, name)?,
            None => {
                let hits: Vec<usize> = (0..n)
                    .filter(|&d| cube.dimensions()[d].kind() == DimensionKind::Measures)
                    .collect();
                match hits.as_slice() {
                    [d] => *d,
                    _ => {
                        return Err(Error::Parse(format!(
                            "import '{}': [values] needs options.value_dimension (the cube has {} measure dimensions)",
                            spec.name,
                            hits.len()
                        )));
                    }
                }
            }
        };
        set(d, Source::Values, "values")?;
        for (member, column) in &spec.values {
            let m = cube.dimensions()[d].member(member)?;
            value_cols.push((find_col(headers, column, &spec.name)?, m, column.clone()));
        }
    }
    let value_col = match (&spec.value_column, value_cols.is_empty()) {
        (Some(c), true) => Some(find_col(headers, c, &spec.name)?),
        (Some(_), false) => {
            return Err(Error::Parse(format!(
                "import '{}': use either [values] or value_column",
                spec.name
            )));
        }
        (None, true) => {
            return Err(Error::Parse(format!(
                "import '{}': no [values] and no value_column given",
                spec.name
            )));
        }
        (None, false) => None,
    };
    let missing: Vec<&str> = (0..n)
        .filter(|&d| sources[d].is_none())
        .map(|d| cube.dimensions()[d].name())
        .collect();
    if !missing.is_empty() {
        return Err(Error::Parse(format!(
            "import '{}': dimension(s) {} not mapped (use [map], [fixed] or [values])",
            spec.name,
            missing.join(", ")
        )));
    }
    Ok(Plan {
        sources: sources.into_iter().flatten().collect(),
        value_cols,
        value_col,
        decimal: spec.decimal()?,
    })
}

fn lookup(dim: &Dimension, text: &str, alias_ok: bool) -> Option<MemberId> {
    let m = dim.resolve(text)?;
    (alias_ok
        || dim
            .member_name(m)
            .is_some_and(|n| n.eq_ignore_ascii_case(text)))
    .then_some(m)
}

/// Runs an import against the cube. `create_members` is called with the dimension index and the
/// names of unknown members to create (policy `create_under`); it must return the edited
/// dimension *after* it was installed in the cube (the persistence layer does that). Without it,
/// that policy fails with a clear message.
///
/// Everything is validated first. If any row is rejected under the `reject` policy, nothing is
/// written. Otherwise the cube is changed by one operation (undoable as a whole).
pub fn run_import(
    cube: &mut Cube,
    spec: &ImportSpec,
    kind: SourceKind,
    data: &[u8],
    ctx: &WriteContext,
    dry_run: bool,
    create_members: Option<CreateMembers>,
) -> Result<ImportReport> {
    run_import_inner(cube, spec, kind, data, ctx, dry_run, create_members).map(|(report, _)| report)
}

/// The reading half of an import, for callers that want to keep the exclusive lock short. It
/// reads and resolves the whole file and checks the write completely (rules, permissions,
/// conflicts), all with shared access: other users keep reading and writing the cube meanwhile.
/// [`apply_prepared`] then writes in one short step. Not usable with `create_under` (creating
/// members changes the model); for that policy this returns an error and the caller runs
/// [`run_import`] instead.
pub fn prepare_import(
    cube: &Cube,
    spec: &ImportSpec,
    kind: SourceKind,
    data: &[u8],
    ctx: &WriteContext,
) -> Result<PreparedImport> {
    if matches!(spec.options.unknown_member, UnknownPolicy::CreateUnder(_)) {
        return Err(Error::InvalidOperation(
            "an import that creates members changes the model; it cannot be prepared separately"
                .into(),
        ));
    }
    let Resolved {
        mut report, write, ..
    } = resolve_import(cube, spec, kind, data, true, false)?;
    report.dry_run = true;
    report.generation = cube.generation();
    let write = match write {
        Some(w) => {
            let outcome = cube.check_bulk(
                ctx,
                BulkWrite {
                    dry_run: true,
                    ..w.clone()
                },
            )?;
            report.cells_created = outcome.created;
            report.cells_updated = outcome.updated;
            report.cells_deleted = outcome.deleted;
            report.cells_unchanged = outcome.unchanged;
            w
        }
        None => BulkWrite {
            clear: None,
            cells: Vec::new(),
            mode: BulkMode::Overwrite,
            dry_run: false,
        },
    };
    Ok(PreparedImport {
        write,
        shape: shape_of(cube),
        report,
    })
}

/// Writes a prepared import as one operation. Fails with a conflict if the model or the cells
/// changed since [`prepare_import`], because the resolved member ids may no longer be right.
pub fn apply_prepared(
    cube: &mut Cube,
    prepared: PreparedImport,
    ctx: &WriteContext,
) -> Result<ImportReport> {
    let mut report = prepared.report;
    if report.failed.is_some() {
        return Ok(report);
    }
    if shape_of(cube) != prepared.shape {
        return Err(Error::Conflict(
            "the model changed while the import was prepared; please run it again".into(),
        ));
    }
    let mut write = prepared.write;
    write.dry_run = false;
    let outcome = cube.bulk_write(ctx, write)?;
    report.dry_run = false;
    report.cells_created = outcome.created;
    report.cells_updated = outcome.updated;
    report.cells_deleted = outcome.deleted;
    report.cells_unchanged = outcome.unchanged;
    if let Some(r) = outcome.result {
        report.op_id = r.op_id;
        report.generation = r.generation;
    }
    Ok(report)
}

/// Result of reading and resolving a file against a cube (the cube is not changed).
struct Resolved {
    report: ImportReport,
    write: Option<BulkWrite>,
    /// Unknown members per dimension index (policy `create_under`).
    unknown: BTreeMap<usize, Vec<String>>,
}

/// Pass 1 and 2 of an import: reads the rows, resolves member names, collects rejections. With
/// the policy `create_under` unknown members are *collected* and not resolved (the caller creates
/// them first and resolves again).
fn resolve_import(
    cube: &Cube,
    spec: &ImportSpec,
    kind: SourceKind,
    data: &[u8],
    dry_run: bool,
    skip_unknown: bool,
) -> Result<Resolved> {
    let mut report = ImportReport {
        spec: spec.name.clone(),
        dry_run,
        generation: cube.generation(),
        ..Default::default()
    };
    let table = read_table(kind, data, &spec.source, spec.delimiter()?)?;
    let plan = make_plan(cube, spec, &table.headers)?;
    let n = cube.dimensions().len();

    // Pass 1: read rows, resolve members; collect unknown ones.
    struct Raw {
        line: u64,
        names: Vec<Option<String>>,
        values: Vec<(MemberId, f64)>,
    }
    let mut rows: Vec<Raw> = Vec::new();
    let mut unknown: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut seen_unknown: HashSet<(usize, String)> = HashSet::new();
    let policy = &spec.options.unknown_member;
    let reject = |report: &mut ImportReport, line: u64, reason: String| {
        report.rows_rejected += 1;
        if report.rejected.len() < MAX_LISTED {
            report.rejected.push(RejectedRow { line, reason });
        }
    };
    for row in table.rows {
        let (line, rec) = row?;
        report.rows_read += 1;
        let mut names = vec![None; n];
        let mut bad = None;
        for (d, src) in plan.sources.iter().enumerate() {
            if let Source::Column(c, alias) = src {
                let text = rec.get(*c).unwrap_or("").trim();
                if text.is_empty() {
                    bad = Some(format!("empty {}", cube.dimensions()[d].name()));
                    break;
                }
                if lookup(&cube.dimensions()[d], text, *alias).is_none() {
                    match policy {
                        UnknownPolicy::CreateUnder(_) => {
                            if seen_unknown.insert((d, text.to_lowercase())) {
                                unknown.entry(d).or_default().push(text.to_string());
                            }
                        }
                        _ => {
                            bad = Some(format!(
                                "unknown member '{text}' in dimension '{}'",
                                cube.dimensions()[d].name()
                            ));
                            break;
                        }
                    }
                }
                names[d] = Some(text.to_string());
            }
        }
        let mut values = Vec::new();
        if bad.is_none() {
            let numbers: Vec<(MemberId, &str, &str)> = match plan.value_col {
                Some(c) => vec![(0, rec.get(c).unwrap_or(""), "value")],
                None => plan
                    .value_cols
                    .iter()
                    .map(|(c, m, name)| (*m, rec.get(*c).unwrap_or(""), name.as_str()))
                    .collect(),
            };
            for (m, text, what) in numbers {
                if text.trim().is_empty() {
                    continue; // an empty value cell is no cell
                }
                match parse_number(text, plan.decimal) {
                    Some(v) => values.push((m, v)),
                    None => {
                        bad = Some(format!("'{text}' in column '{what}' is not a number"));
                        break;
                    }
                }
            }
        }
        match bad {
            Some(reason) => reject(&mut report, line, reason),
            None => rows.push(Raw {
                line,
                names,
                values,
            }),
        }
    }

    // Check the parent for unknown members (policy create_under); creating them is the caller's job.
    if let UnknownPolicy::CreateUnder(parent) = policy {
        for (d, names) in &unknown {
            let dim = &cube.dimensions()[*d];
            match dim.resolve(parent) {
                None => {
                    report.failed = Some(format!(
                        "the member '{parent}' for unknown members does not exist in dimension '{}'",
                        dim.name()
                    ));
                }
                Some(p) => {
                    // A base member that already holds cells would turn into an aggregate and lose them.
                    let holds_cells = dim
                        .leaf_ordinal(p)
                        .is_some_and(|o| cube.store().count_of_leaf(*d, o) > 0);
                    if holds_cells {
                        report.failed = Some(format!(
                            "'{parent}' in dimension '{}' holds data and cannot become the parent of new members; name an aggregate or an empty member",
                            dim.name()
                        ));
                    }
                }
            }
            report
                .members_created
                .extend(names.iter().map(|n| format!("{}: {n}", dim.name())));
        }
        if report.failed.is_some() {
            return Ok(Resolved {
                report,
                write: None,
                unknown: BTreeMap::new(),
            });
        }
    }

    // Pass 2: resolve to base addresses.
    let dry_unknown = skip_unknown && matches!(policy, UnknownPolicy::CreateUnder(_));
    let mut cells: Vec<(Vec<MemberId>, f64)> = Vec::new();
    let mut covered: Vec<HashSet<MemberId>> = vec![HashSet::new(); n];
    for row in rows {
        let mut addr = vec![0; n];
        let mut bad = None;
        for (d, src) in plan.sources.iter().enumerate() {
            let dim = &cube.dimensions()[d];
            addr[d] = match src {
                Source::Fixed(m) => *m,
                Source::Values => 0,
                Source::Column(_, alias) => {
                    let text = row.names[d].as_deref().unwrap_or("");
                    match lookup(dim, text, *alias) {
                        Some(m) => m,
                        None if dry_unknown => 0,
                        None => {
                            bad = Some(format!(
                                "unknown member '{text}' in dimension '{}'",
                                dim.name()
                            ));
                            break;
                        }
                    }
                }
            };
        }
        if bad.is_none() && !dry_unknown {
            for (d, src) in plan.sources.iter().enumerate() {
                if !matches!(src, Source::Values) && !cube.dimensions()[d].is_leaf(addr[d]) {
                    bad = Some(format!(
                        "'{}' in dimension '{}' is an aggregate; only base members can be loaded",
                        cube.dimensions()[d].member_name(addr[d]).unwrap_or("?"),
                        cube.dimensions()[d].name()
                    ));
                    break;
                }
            }
        }
        if let Some(reason) = bad {
            reject(&mut report, row.line, reason);
            continue;
        }
        for (m, v) in row.values {
            let mut full = addr.clone();
            for (d, src) in plan.sources.iter().enumerate() {
                if matches!(src, Source::Values) {
                    full[d] = if plan.value_col.is_some() { addr[d] } else { m };
                }
            }
            if !dry_unknown {
                for (d, member) in full.iter().enumerate() {
                    covered[d].insert(*member);
                }
                if let Some(d) = plan
                    .sources
                    .iter()
                    .position(|s| matches!(s, Source::Values))
                    && !cube.dimensions()[d].is_leaf(full[d])
                {
                    reject(
                        &mut report,
                        row.line,
                        format!(
                            "'{}' is an aggregate; only base members can be loaded",
                            cube.dimensions()[d].member_name(full[d]).unwrap_or("?")
                        ),
                    );
                    continue;
                }
            }
            report.cells_read += 1;
            cells.push((full, v));
        }
    }

    if report.rows_rejected > 0 && matches!(policy, UnknownPolicy::Reject) {
        report.failed = Some(format!(
            "{} row(s) rejected, nothing was loaded",
            report.rows_rejected
        ));
        return Ok(Resolved {
            report,
            write: None,
            unknown: BTreeMap::new(),
        });
    }
    if dry_unknown {
        return Ok(Resolved {
            report,
            write: None,
            unknown,
        });
    }

    let clear = (spec.options.mode == LoadMode::ReplaceArea).then(|| {
        covered
            .into_iter()
            .map(|s| {
                let mut v: Vec<MemberId> = s.into_iter().collect();
                v.sort_unstable();
                v
            })
            .collect()
    });
    let write = BulkWrite {
        clear,
        cells,
        mode: if spec.options.mode == LoadMode::Add {
            BulkMode::Add
        } else {
            BulkMode::Overwrite
        },
        dry_run,
    };
    Ok(Resolved {
        report,
        write: Some(write),
        unknown,
    })
}

fn run_import_inner(
    cube: &mut Cube,
    spec: &ImportSpec,
    kind: SourceKind,
    data: &[u8],
    ctx: &WriteContext,
    dry_run: bool,
    create_members: Option<CreateMembers>,
) -> Result<(ImportReport, Option<BulkWrite>)> {
    let creates = matches!(spec.options.unknown_member, UnknownPolicy::CreateUnder(_));
    // First look. With `create_under` and a real run, unknown members are only collected.
    let first = resolve_import(cube, spec, kind, data, dry_run, creates)?;
    let mut resolved = first;
    if creates {
        let parent = match &spec.options.unknown_member {
            UnknownPolicy::CreateUnder(p) => p.clone(),
            _ => unreachable!("checked above"),
        };
        if resolved.report.failed.is_some() {
            return Ok((resolved.report, None));
        }
        if !dry_run && !resolved.unknown.is_empty() {
            let Some(create) = create_members else {
                return Err(Error::InvalidOperation(
                    "creating members during an import needs a database (use the persistent import)".into(),
                ));
            };
            for (d, names) in std::mem::take(&mut resolved.unknown) {
                create(
                    cube,
                    d,
                    names.into_iter().map(|n| (n, parent.clone())).collect(),
                )?;
            }
        }
        if !dry_run {
            // The model changed: resolve once more, now all members exist.
            let members_created = std::mem::take(&mut resolved.report.members_created);
            resolved = resolve_import(cube, spec, kind, data, false, false)?;
            resolved.report.members_created = members_created;
        }
    }
    let Resolved {
        mut report, write, ..
    } = resolved;
    let Some(write) = write else {
        return Ok((report, None));
    };
    report.generation = cube.generation();
    let outcome: BulkOutcome = cube.bulk_write(
        ctx,
        BulkWrite {
            dry_run,
            ..write.clone()
        },
    )?;
    report.cells_created = outcome.created;
    report.cells_updated = outcome.updated;
    report.cells_deleted = outcome.deleted;
    report.cells_unchanged = outcome.unchanged;
    if let Some(r) = outcome.result {
        report.op_id = r.op_id;
        report.generation = r.generation;
    }
    Ok((report, dry_run.then_some(write)))
}

/// Reads the source file named in the specification (relative to `base_dir`) and runs the import.
pub fn run_import_file(
    cube: &mut Cube,
    spec: &ImportSpec,
    base_dir: &Path,
    ctx: &WriteContext,
    dry_run: bool,
    create_members: Option<CreateMembers>,
) -> Result<ImportReport> {
    let file =
        spec.source.file.as_deref().ok_or_else(|| {
            Error::Parse(format!("import '{}': source.file is missing", spec.name))
        })?;
    let path = base_dir.join(file);
    let data = std::fs::read(&path)
        .map_err(|e| Error::Storage(format!("cannot read '{}': {e}", path.display())))?;
    run_import(
        cube,
        spec,
        SourceKind::from_path(&path),
        &data,
        ctx,
        dry_run,
        create_members,
    )
}
