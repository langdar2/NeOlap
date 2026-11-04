//! Python module `neolap._native`: local access to a database file.
//!
//! Everything with logic lives in `neolap-core`; this file only translates errors into Python
//! exceptions, releases the GIL around engine work and hands results over column-wise (see
//! `Columns`), so a large area reaches pandas without one Python object per cell. The user-facing
//! API (`neolap.open`, `neolap.connect`, `Cube`, `Area`) is plain Python in `python/neolap/`, so
//! the local and the remote backend share it by construction.

use std::collections::HashMap;
use std::sync::Arc;

use neolap_core::cube::{Input, WriteContext};
use neolap_core::exchange::{self, Cell, ExportSpec, ImportSpec, Layout, SourceKind};
use neolap_core::{Database, Error};
use parking_lot::Mutex;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};

pyo3::create_exception!(
    _native,
    NeolapError,
    pyo3::exceptions::PyException,
    "Base class of all NeOlap errors."
);
pyo3::create_exception!(
    _native,
    NotFoundError,
    NeolapError,
    "A cube, dimension or member does not exist."
);
pyo3::create_exception!(
    _native,
    ConflictError,
    NeolapError,
    "A cell was changed by someone else in the meantime."
);
pyo3::create_exception!(
    _native,
    PermissionError,
    NeolapError,
    "The user may not do this."
);
pyo3::create_exception!(
    _native,
    InputError,
    NeolapError,
    "The input or the request is invalid."
);
pyo3::create_exception!(
    _native,
    LockedError,
    NeolapError,
    "The database file is in use by another process."
);
pyo3::create_exception!(
    _native,
    StorageError,
    NeolapError,
    "The database file could not be read or written."
);

/// Maps an engine error to the exception class a Python user would catch.
fn err(e: Error) -> PyErr {
    let msg = e.to_string();
    match e {
        Error::NotFound { .. } => NotFoundError::new_err(msg),
        Error::WriteConflict { user, cell, value } => {
            let ex = ConflictError::new_err(msg);
            Python::attach(|py| {
                let v = ex.value(py);
                let _ = v.setattr("user", user);
                let _ = v.setattr("cell", cell);
                let _ = v.setattr("value", value);
            });
            ex
        }
        Error::Denied(_) => PermissionError::new_err(msg),
        Error::Locked(_) => LockedError::new_err(msg),
        Error::Storage(_) => StorageError::new_err(msg),
        Error::Conflict(_) => ConflictError::new_err(msg),
        Error::Parse(_) | Error::InvalidOperation(_) | Error::Cycle { .. } => {
            InputError::new_err(msg)
        }
    }
}

struct Lookup<'a>(&'a Database);

impl neolap_core::CubeProvider for Lookup<'_> {
    fn cube(&self, name: &str) -> Option<&neolap_core::Cube> {
        self.0.cube(name)
    }
}

/// An open database file. Opening is exclusive: no server and no second process may use it.
#[pyclass(module = "neolap._native")]
struct LocalDatabase {
    db: Arc<Mutex<Option<Database>>>,
    user: String,
}

/// Runs `f` on the open database; a closed one is an error, not a crash.
fn with_db<T>(
    db: &Mutex<Option<Database>>,
    f: impl FnOnce(&mut Database) -> Result<T, Error>,
) -> Result<T, Error> {
    match db.lock().as_mut() {
        Some(d) => f(d),
        None => Err(Error::InvalidOperation("the database is closed".into())),
    }
}

#[pymethods]
impl LocalDatabase {
    #[staticmethod]
    #[pyo3(signature = (path, user = "python".to_string()))]
    fn open(py: Python<'_>, path: String, user: String) -> PyResult<Self> {
        let db = py.detach(|| Database::open(&path)).map_err(err)?;
        Ok(LocalDatabase {
            db: Arc::new(Mutex::new(Some(db))),
            user,
        })
    }

    #[staticmethod]
    #[pyo3(signature = (path, user = "python".to_string()))]
    fn create(py: Python<'_>, path: String, user: String) -> PyResult<Self> {
        let db = py.detach(|| Database::create(&path)).map_err(err)?;
        Ok(LocalDatabase {
            db: Arc::new(Mutex::new(Some(db))),
            user,
        })
    }

    /// Releases the file. Afterwards every call fails with `InputError`; the file can be opened again.
    fn close(&self, py: Python<'_>) {
        let db = self.db.clone();
        // dropping the database closes the connection and releases the lock file
        py.detach(move || drop(db.lock().take()));
    }

    fn cubes(&self) -> PyResult<Vec<String>> {
        with_db(&self.db, |d| {
            Ok(d.cubes().map(|c| c.name().to_string()).collect())
        })
        .map_err(err)
    }

    fn dimensions(&self, cube: &str) -> PyResult<Vec<String>> {
        with_db(&self.db, |d| {
            let c = d.cube(cube).ok_or_else(|| Error::NotFound {
                kind: "cube",
                name: cube.to_string(),
            })?;
            Ok(c.dimensions()
                .iter()
                .map(|x| x.name().to_string())
                .collect())
        })
        .map_err(err)
    }

    fn generation(&self, cube: &str) -> PyResult<u64> {
        with_db(&self.db, |d| {
            d.cube(cube)
                .map(|c| c.generation())
                .ok_or_else(|| Error::NotFound {
                    kind: "cube",
                    name: cube.to_string(),
                })
        })
        .map_err(err)
    }

    /// Value of one cell (base or aggregate), `None` if empty. Rules are applied when `rules`.
    #[pyo3(signature = (cube, members, rules = true))]
    fn get(
        &self,
        py: Python<'_>,
        cube: &str,
        members: Vec<String>,
        rules: bool,
    ) -> PyResult<Option<f64>> {
        let db = self.db.clone();
        let cube = cube.to_string();
        py.detach(move || {
            with_db(&db, |d| {
                let c = d.cube(&cube).ok_or_else(|| Error::NotFound {
                    kind: "cube",
                    name: cube.clone(),
                })?;
                let refs: Vec<&str> = members.iter().map(String::as_str).collect();
                let addr = c.address(&refs)?;
                if rules {
                    match c.cell(&addr, Some(&Lookup(d)))? {
                        neolap_core::CellValue::Num(v) => Ok(Some(v)),
                        neolap_core::CellValue::Empty => Ok(None),
                        neolap_core::CellValue::Text(t) => Err(Error::InvalidOperation(format!(
                            "the cell holds the text '{t}'"
                        ))),
                        neolap_core::CellValue::Error(e) => {
                            Err(Error::InvalidOperation(format!("the cell is {e}")))
                        }
                    }
                } else {
                    c.value(&addr)
                }
            })
        })
        .map_err(err)
    }

    /// Writes a value in the splash syntax (`500`, `#500`, `+50`, `+5%`, `@Ref:500`, empty = delete).
    /// Returns `(cells_changed, generation)`.
    #[pyo3(signature = (cube, members, value, expected_generation = None))]
    fn write(
        &self,
        py: Python<'_>,
        cube: &str,
        members: Vec<String>,
        value: String,
        expected_generation: Option<u64>,
    ) -> PyResult<(usize, u64)> {
        let db = self.db.clone();
        let (cube, user) = (cube.to_string(), self.user.clone());
        py.detach(move || {
            with_db(&db, |d| {
                let c = d.cube_mut(&cube).ok_or_else(|| Error::NotFound {
                    kind: "cube",
                    name: cube.clone(),
                })?;
                let refs: Vec<&str> = members.iter().map(String::as_str).collect();
                let addr = c.address(&refs)?;
                let mut ctx = WriteContext::new(&user);
                ctx.expected_generation = expected_generation;
                let r = c.write(&ctx, &addr, Input::parse(&value)?)?;
                Ok((r.cells_changed, r.generation))
            })
        })
        .map_err(err)
    }

    fn undo(&self, py: Python<'_>, cube: &str) -> PyResult<(usize, u64)> {
        let db = self.db.clone();
        let (cube, user) = (cube.to_string(), self.user.clone());
        py.detach(move || {
            with_db(&db, |d| {
                let c = d.cube_mut(&cube).ok_or_else(|| Error::NotFound {
                    kind: "cube",
                    name: cube.clone(),
                })?;
                let r = c.undo(&WriteContext::new(&user))?;
                Ok((r.cells_changed, r.generation))
            })
        })
        .map_err(err)
    }

    /// An area in long format as columns: see `Columns`. `area` maps dimension → member names;
    /// dimensions that are not named give all base members (all members with `aggregates`).
    #[pyo3(signature = (cube, area, aggregates = false, rules = false, wide = None))]
    fn area(
        &self,
        py: Python<'_>,
        cube: &str,
        area: Vec<(String, Vec<String>)>,
        aggregates: bool,
        rules: bool,
        wide: Option<String>,
    ) -> PyResult<Columns> {
        let db = self.db.clone();
        let cube = cube.to_string();
        py.detach(move || {
            with_db(&db, |d| {
                let c = d.cube(&cube).ok_or_else(|| Error::NotFound {
                    kind: "cube",
                    name: cube.clone(),
                })?;
                let spec = ExportSpec {
                    area,
                    layout: wide.map_or(Layout::Long, |dimension| Layout::Wide { dimension }),
                    aggregates,
                    with_rules: rules,
                    suppress_empty: true,
                };
                Ok(Columns::from_table(exchange::export_table(
                    c,
                    &spec,
                    Some(&Lookup(d)),
                )?))
            })
        })
        .map_err(err)
    }

    /// Runs an import specification (TOML) on `data` (the file's bytes). Returns the report as a dict.
    #[pyo3(signature = (spec, data, kind = "csv".to_string(), dry_run = false))]
    fn load(
        &self,
        py: Python<'_>,
        spec: String,
        data: Vec<u8>,
        kind: String,
        dry_run: bool,
    ) -> PyResult<Py<PyDict>> {
        let db = self.db.clone();
        let user = self.user.clone();
        let report = py
            .detach(move || {
                let spec = ImportSpec::from_toml(&spec)?;
                let kind = if kind == "xlsx" {
                    SourceKind::Xlsx
                } else {
                    SourceKind::Csv
                };
                with_db(&db, |d| {
                    d.run_import(&spec, kind, &data, &WriteContext::new(&user), dry_run)
                })
            })
            .map_err(err)?;
        let d = PyDict::new(py);
        d.set_item("import", report.spec)?;
        d.set_item("dry_run", report.dry_run)?;
        d.set_item("rows_read", report.rows_read)?;
        d.set_item("cells_read", report.cells_read)?;
        d.set_item("cells_created", report.cells_created)?;
        d.set_item("cells_updated", report.cells_updated)?;
        d.set_item("cells_deleted", report.cells_deleted)?;
        d.set_item("cells_unchanged", report.cells_unchanged)?;
        d.set_item("rows_rejected", report.rows_rejected)?;
        d.set_item(
            "rejected",
            report
                .rejected
                .iter()
                .map(|r| (r.line, r.reason.clone()))
                .collect::<Vec<_>>(),
        )?;
        d.set_item("members_created", report.members_created)?;
        d.set_item("failed", report.failed)?;
        Ok(d.unbind())
    }

    fn rules(&self, cube: &str) -> PyResult<String> {
        with_db(&self.db, |d| {
            d.cube(cube)
                .map(|c| c.rules_text().to_string())
                .ok_or_else(|| Error::NotFound {
                    kind: "cube",
                    name: cube.to_string(),
                })
        })
        .map_err(err)
    }

    fn backup(&self, py: Python<'_>, target: String) -> PyResult<()> {
        let db = self.db.clone();
        py.detach(move || with_db(&db, |d| d.backup(&target)))
            .map_err(err)
    }
}

/// A long-format table as columns. Dimension columns are categorical (`codes` + a table of
/// `names`), values are one buffer of little-endian `f64` where NaN means "empty". Building a
/// pandas `DataFrame` from this needs no Python object per cell.
#[pyclass(module = "neolap._native")]
struct Columns {
    #[pyo3(get)]
    headers: Vec<String>,
    /// Per header: the distinct names (empty for numeric columns).
    names: Vec<Vec<String>>,
    /// Per header: `u32` codes (dimension columns) or `f64` values (numeric columns), little-endian.
    data: Vec<Vec<u8>>,
    /// Per header: `true` for numeric columns.
    numeric: Vec<bool>,
    #[pyo3(get)]
    n_rows: usize,
}

impl Columns {
    fn from_table(t: exchange::ExportTable) -> Columns {
        let n_cols = t.headers.len();
        let n_rows = t.rows.len();
        // the kind of a column comes from its position, not from what it happens to hold: the
        // first columns are member names (even a member called "2026"), the rest are values (even
        // a measure that is empty everywhere). The remote client uses the same rule.
        let numeric: Vec<bool> = (0..n_cols).map(|c| c >= t.n_name_columns).collect();
        let mut names: Vec<Vec<String>> = vec![Vec::new(); n_cols];
        let mut data: Vec<Vec<u8>> = vec![Vec::new(); n_cols];
        let mut index: Vec<HashMap<String, u32>> = vec![HashMap::new(); n_cols];
        for c in 0..n_cols {
            data[c].reserve(n_rows * if numeric[c] { 8 } else { 4 });
        }
        for row in &t.rows {
            for (c, cell) in row.iter().enumerate() {
                if numeric[c] {
                    let v = match cell {
                        Cell::Num(v) => *v,
                        _ => f64::NAN,
                    };
                    data[c].extend_from_slice(&v.to_le_bytes());
                } else {
                    let text = match cell {
                        Cell::Text(s) => s.as_str(),
                        Cell::Num(_) | Cell::Empty => "",
                    };
                    let next = names[c].len() as u32;
                    let code = *index[c].entry(text.to_string()).or_insert_with(|| {
                        names[c].push(text.to_string());
                        next
                    });
                    data[c].extend_from_slice(&code.to_le_bytes());
                }
            }
        }
        Columns {
            headers: t.headers,
            names,
            data,
            numeric,
            n_rows,
        }
    }
}

#[pymethods]
impl Columns {
    fn is_numeric(&self, i: usize) -> PyResult<bool> {
        self.numeric
            .get(i)
            .copied()
            .ok_or_else(|| PyValueError::new_err("no such column"))
    }

    fn names<'py>(&self, py: Python<'py>, i: usize) -> PyResult<Bound<'py, PyList>> {
        let n = self
            .names
            .get(i)
            .ok_or_else(|| PyValueError::new_err("no such column"))?;
        PyList::new(py, n)
    }

    /// The raw buffer of column `i` (`u32` codes or `f64` values, little-endian).
    fn buffer<'py>(&self, py: Python<'py>, i: usize) -> PyResult<Bound<'py, PyBytes>> {
        let d = self
            .data
            .get(i)
            .ok_or_else(|| PyValueError::new_err("no such column"))?;
        Ok(PyBytes::new(py, d))
    }

    fn __len__(&self) -> usize {
        self.n_rows
    }
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", neolap_core::VERSION)?;
    m.add_class::<LocalDatabase>()?;
    m.add_class::<Columns>()?;
    let py = m.py();
    m.add("NeolapError", py.get_type::<NeolapError>())?;
    m.add("NotFoundError", py.get_type::<NotFoundError>())?;
    m.add("ConflictError", py.get_type::<ConflictError>())?;
    m.add("PermissionError", py.get_type::<PermissionError>())?;
    m.add("InputError", py.get_type::<InputError>())?;
    m.add("LockedError", py.get_type::<LockedError>())?;
    m.add("StorageError", py.get_type::<StorageError>())?;
    Ok(())
}
