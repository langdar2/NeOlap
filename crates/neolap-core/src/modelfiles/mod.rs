//! The model as readable files, for versioning in Git (`neolap model export/import`).
//!
//! ```text
//! <dir>/
//!   dimensions/<name>.csv     child;parent;weight;alias;<attribute>…   (one row per edge)
//!   dimensions/<name>.toml    kind, subsets
//!   cubes/<name>.toml         dimensions of the cube
//!   rules/<name>.rules        rule text
//!   imports/<name>.toml       import specifications
//! ```
//!
//! Only the model is exported, never cell data. The output is deterministic (stable order, no
//! timestamps), so an unchanged model produces an unchanged directory.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dimension::{
    AttrValue, Dimension, DimensionEditor, DimensionKind, ImportMode, Subset,
    import_parent_child_with,
};
use crate::persist::Database;
use crate::{Error, MemberId, Result};

#[derive(Serialize, Deserialize)]
struct DimensionMeta {
    kind: DimensionKind,
    /// Subsets by member *name*, so the file does not depend on internal ids.
    #[serde(default)]
    subsets: Vec<SubsetFile>,
}

#[derive(Serialize, Deserialize)]
struct SubsetFile {
    name: String,
    members: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct CubeFile {
    dimensions: Vec<String>,
}

/// What an import changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelImportReport {
    pub dimensions: Vec<String>,
    pub cubes: Vec<String>,
    pub rules: Vec<String>,
    pub import_specs: Vec<String>,
}

fn file_name(name: &str) -> Result<String> {
    if name.is_empty()
        || name.contains(['/', '\\', '\0'])
        || name == "."
        || name == ".."
        || name.starts_with('.')
    {
        return Err(Error::InvalidOperation(format!(
            "'{name}' cannot be used as a file name"
        )));
    }
    Ok(name.to_string())
}

fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, text)?;
    Ok(())
}

fn csv_field(s: &str, delimiter: char) -> String {
    if s.contains([delimiter, '"', '\n', '\r']) || s.starts_with(' ') || s.ends_with(' ') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn attr_text(v: &AttrValue) -> String {
    match v {
        AttrValue::Number(n) => n.to_string(),
        AttrValue::Text(t) => t.clone(),
    }
}

/// Members in a stable, meaningful order: roots first as created, children in their order,
/// each member once (a member below several parents is listed with every edge).
fn ordered(dim: &Dimension) -> Vec<MemberId> {
    let mut out = Vec::with_capacity(dim.len());
    let mut seen = vec![false; dim.len_slots()];
    for &root in dim.roots() {
        let mut stack = vec![root];
        while let Some(m) = stack.pop() {
            if std::mem::replace(&mut seen[m as usize], true) {
                continue;
            }
            out.push(m);
            stack.extend(dim.children(m).iter().rev().map(|c| c.0));
        }
    }
    out
}

/// CSV of one dimension: one row per edge; members without parent get one row with an empty parent.
pub fn dimension_csv(dim: &Dimension) -> String {
    let attrs: Vec<&str> = dim.attribute_names().collect();
    let mut out = String::from("child;parent;weight;alias");
    for a in &attrs {
        out.push(';');
        out.push_str(&csv_field(a, ';'));
    }
    out.push('\n');
    for m in ordered(dim) {
        let name = dim.member_name(m).unwrap_or_default();
        let aliases = dim.aliases(m).join("|");
        let parents = dim.parents(m);
        let rows: Vec<(String, String)> = if parents.is_empty() {
            vec![(String::new(), String::new())]
        } else {
            parents
                .iter()
                .map(|&(p, w)| {
                    (
                        dim.member_name(p).unwrap_or_default().to_string(),
                        if w == 1.0 {
                            String::new()
                        } else {
                            w.to_string()
                        },
                    )
                })
                .collect()
        };
        for (i, (parent, weight)) in rows.into_iter().enumerate() {
            out.push_str(&csv_field(name, ';'));
            out.push(';');
            out.push_str(&csv_field(&parent, ';'));
            out.push(';');
            out.push_str(&weight);
            out.push(';');
            // alias and attributes belong to the member, so only the first of its rows carries them
            if i == 0 {
                out.push_str(&csv_field(&aliases, ';'));
            }
            for a in &attrs {
                out.push(';');
                if i == 0
                    && let Some(v) = dim.attribute(a, m)
                {
                    out.push_str(&csv_field(&attr_text(v), ';'));
                }
            }
            out.push('\n');
        }
    }
    out
}

/// Writes the model into `dir` (created if needed). Files of objects that no longer exist are
/// removed from the managed subdirectories, so the directory mirrors the model exactly.
pub fn export_model(db: &Database, dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    let mut keep: BTreeMap<&str, Vec<PathBuf>> = BTreeMap::new();
    for name in db.dimension_names() {
        let dim = db.dimension(name).expect("listed");
        let base = dir.join("dimensions").join(file_name(name)?);
        write(&base.with_extension("csv"), &dimension_csv(dim))?;
        let meta = DimensionMeta {
            kind: dim.kind(),
            subsets: dim
                .subsets()
                .iter()
                .map(|s| SubsetFile {
                    name: s.name.clone(),
                    members: s
                        .members
                        .iter()
                        .filter_map(|&m| dim.member_name(m))
                        .map(str::to_string)
                        .collect(),
                })
                .collect(),
        };
        write(
            &base.with_extension("toml"),
            &toml::to_string_pretty(&meta).map_err(|e| Error::Storage(e.to_string()))?,
        )?;
        keep.entry("dimensions")
            .or_default()
            .extend([base.with_extension("csv"), base.with_extension("toml")]);
    }
    for cube in db.cubes() {
        let path = dir
            .join("cubes")
            .join(format!("{}.toml", file_name(cube.name())?));
        let file = CubeFile {
            dimensions: cube
                .dimensions()
                .iter()
                .map(|d| d.name().to_string())
                .collect(),
        };
        write(
            &path,
            &toml::to_string_pretty(&file).map_err(|e| Error::Storage(e.to_string()))?,
        )?;
        keep.entry("cubes").or_default().push(path);
        if !cube.rules_text().trim().is_empty() {
            let path = dir
                .join("rules")
                .join(format!("{}.rules", file_name(cube.name())?));
            let text = cube.rules_text();
            write(
                &path,
                &if text.ends_with('\n') {
                    text.to_string()
                } else {
                    format!("{text}\n")
                },
            )?;
            keep.entry("rules").or_default().push(path);
        }
    }
    for name in db.import_spec_names()? {
        let path = dir
            .join("imports")
            .join(format!("{}.toml", file_name(&name)?));
        write(&path, &db.import_spec(&name)?)?;
        keep.entry("imports").or_default().push(path);
    }
    for sub in ["dimensions", "cubes", "rules", "imports"] {
        let wanted = keep.get(sub).cloned().unwrap_or_default();
        let Ok(entries) = fs::read_dir(dir.join(sub)) else {
            continue;
        };
        for e in entries.flatten() {
            if e.path().is_file() && !wanted.contains(&e.path()) {
                fs::remove_file(e.path())?;
            }
        }
    }
    Ok(())
}

fn read_dir_sorted(dir: &Path, ext: &str) -> Result<Vec<PathBuf>> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut v: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == ext))
        .collect();
    v.sort();
    Ok(v)
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Builds the dimension described by `<name>.csv` and the optional `<name>.toml`. If the
/// dimension exists, it is edited rather than rebuilt: members keep their ids (cells are stored
/// under them), members missing from the file are removed, the structure follows the file.
fn read_dimension(name: &str, csv_path: &Path, existing: Option<&Dimension>) -> Result<Dimension> {
    let meta_path = csv_path.with_extension("toml");
    let meta: DimensionMeta = if meta_path.exists() {
        toml::from_str(&fs::read_to_string(&meta_path)?)
            .map_err(|e| Error::Parse(format!("{}: {e}", meta_path.display())))?
    } else {
        DimensionMeta {
            kind: DimensionKind::Generic,
            subsets: Vec::new(),
        }
    };
    let mut ed: DimensionEditor = match existing {
        Some(old) => {
            let mut ed = old.edit();
            ed.reset_structure();
            ed.set_kind(meta.kind);
            ed
        }
        None => Dimension::new(name, meta.kind).edit(),
    };
    // Members the file does not mention are removed *first*, so that their names are free for
    // aliases and new members of the same file.
    let in_file = names_in_csv(csv_path)?;
    for id in ed.member_ids() {
        if !in_file.contains(&ed.name_of_member(id).to_lowercase()) {
            ed.remove(id)?;
        }
    }
    // The exported CSV lists a member once per edge; the alias and attributes sit on its first row.
    import_parent_child_with(
        &mut ed,
        fs::File::open(csv_path)?,
        b';',
        ImportMode::AddOnly,
        true,
    )
    .map_err(|e| Error::Parse(format!("{}: {e}", csv_path.display())))?;
    for s in meta.subsets {
        let members: Vec<MemberId> = s
            .members
            .iter()
            .map(|m| {
                ed.resolve(m).ok_or_else(|| {
                    Error::Parse(format!(
                        "subset '{}' of '{name}' names the unknown member '{m}'",
                        s.name
                    ))
                })
            })
            .collect::<Result<_>>()?;
        ed.set_subset(&s.name, members)?;
    }
    ed.commit()
}

/// Lower-case names of the `child` column of a dimension CSV.
fn names_in_csv(path: &Path) -> Result<std::collections::HashSet<String>> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(b';')
        .flexible(true)
        .trim(csv::Trim::None)
        .from_path(path)
        .map_err(|e| Error::Parse(format!("{}: {e}", path.display())))?;
    let idx = rdr
        .headers()
        .map_err(|e| Error::Parse(e.to_string()))?
        .iter()
        .position(|h| h.eq_ignore_ascii_case("child"))
        .ok_or_else(|| Error::Parse(format!("{}: column 'child' missing", path.display())))?;
    let mut out = std::collections::HashSet::new();
    for rec in rdr.records() {
        let rec = rec.map_err(|e| Error::Parse(e.to_string()))?;
        out.insert(rec.get(idx).unwrap_or("").to_lowercase());
    }
    Ok(out)
}

/// Reads a model directory into the database. Everything is validated first (dimensions, cube
/// references, rule syntax, import specifications); on any error nothing is changed.
///
/// Dimensions are replaced by the file content: members that no longer exist lose their cells
/// (journaled, like any dimension change). Cubes that exist already must keep their dimensions.
pub fn import_model(db: &mut Database, dir: &Path, user: &str) -> Result<ModelImportReport> {
    if !dir.is_dir() {
        return Err(Error::NotFound {
            kind: "model directory",
            name: dir.display().to_string(),
        });
    }
    let mut dims: Vec<Dimension> = Vec::new();
    for csv in read_dir_sorted(&dir.join("dimensions"), "csv")? {
        let name = stem(&csv);
        let existing = db.dimension(&name).cloned();
        dims.push(read_dimension(&name, &csv, existing.as_deref())?);
    }
    let mut cubes: Vec<(String, Vec<String>)> = Vec::new();
    for p in read_dir_sorted(&dir.join("cubes"), "toml")? {
        let f: CubeFile = toml::from_str(&fs::read_to_string(&p)?)
            .map_err(|e| Error::Parse(format!("{}: {e}", p.display())))?;
        cubes.push((stem(&p), f.dimensions));
    }
    let known = |d: &str| {
        dims.iter().any(|x| x.name().eq_ignore_ascii_case(d)) || db.dimension(d).is_some()
    };
    for (cube, ds) in &cubes {
        if let Some(missing) = ds.iter().find(|d| !known(d)) {
            return Err(Error::Parse(format!(
                "cube '{cube}' uses the dimension '{missing}', which is neither in the files nor in the database"
            )));
        }
        if let Some(existing) = db.cube(cube) {
            let have: Vec<&str> = existing.dimensions().iter().map(|d| d.name()).collect();
            let want: Vec<&str> = ds.iter().map(String::as_str).collect();
            if have != want {
                return Err(Error::Conflict(format!(
                    "cube '{cube}' exists with the dimensions [{}]; the files name [{}]. A cube's dimensions cannot be changed",
                    have.join(", "),
                    want.join(", ")
                )));
            }
        }
    }
    let mut rules: Vec<(String, String)> = Vec::new();
    for p in read_dir_sorted(&dir.join("rules"), "rules")? {
        let name = stem(&p);
        if !cubes.iter().any(|c| c.0.eq_ignore_ascii_case(&name)) && db.cube(&name).is_none() {
            return Err(Error::Parse(format!(
                "{}: there is no cube '{name}'",
                p.display()
            )));
        }
        rules.push((name, fs::read_to_string(&p)?));
    }
    let mut specs: Vec<String> = Vec::new();
    for p in read_dir_sorted(&dir.join("imports"), "toml")? {
        let text = fs::read_to_string(&p)?;
        let spec = crate::exchange::ImportSpec::from_toml(&text)
            .map_err(|e| Error::Parse(format!("{}: {e}", p.display())))?;
        if !cubes.iter().any(|c| c.0.eq_ignore_ascii_case(&spec.cube))
            && db.cube(&spec.cube).is_none()
        {
            return Err(Error::Parse(format!(
                "{}: the import names the unknown cube '{}'",
                p.display(),
                spec.cube
            )));
        }
        specs.push(text);
    }

    // Apply. Only objects whose content differs are written, so a re-import of an unchanged
    // directory changes nothing (and journals nothing).
    let mut report = ModelImportReport::default();
    for dim in dims {
        let same = db.dimension(dim.name()).is_some_and(|old| {
            dimension_csv(old) == dimension_csv(&dim)
                && old.kind() == dim.kind()
                && subsets_equal(old, &dim)
        });
        if !same {
            report.dimensions.push(dim.name().to_string());
            db.save_dimension(dim, user)?;
        }
    }
    for (name, ds) in cubes {
        if db.cube(&name).is_none() {
            let refs: Vec<&str> = ds.iter().map(String::as_str).collect();
            db.create_cube(&name, &refs, user)?;
            report.cubes.push(name);
        }
    }
    for (name, text) in rules {
        let current = db
            .cube(&name)
            .map(|c| c.rules_text().trim_end().to_string())
            .unwrap_or_default();
        if current != text.trim_end() {
            db.set_rules(&name, &text, user)?;
            report.rules.push(name);
        }
    }
    for text in specs {
        let spec = crate::exchange::ImportSpec::from_toml(&text)?;
        if db
            .import_spec(&spec.name)
            .map(|t| t.trim_end() != text.trim_end())
            .unwrap_or(true)
        {
            db.save_import_spec(&text, user)?;
            report.import_specs.push(spec.name);
        }
    }
    Ok(report)
}

fn subsets_equal(a: &Dimension, b: &Dimension) -> bool {
    let names = |d: &Dimension| -> Vec<(String, Vec<String>)> {
        d.subsets()
            .iter()
            .map(|Subset { name, members }| {
                (
                    name.clone(),
                    members
                        .iter()
                        .filter_map(|&m| d.member_name(m))
                        .map(str::to_string)
                        .collect(),
                )
            })
            .collect()
    };
    names(a) == names(b)
}

#[cfg(test)]
mod tests;
