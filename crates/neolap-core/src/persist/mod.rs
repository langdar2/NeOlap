//! Persistence: one database = one SQLite file (`*.neolap`), exclusive per process.
//!
//! The file is the source of truth. Every operation is written in its own transaction *before* it
//! is applied in memory (write-through, WAL), so a crash or abort never leaves a half-applied
//! operation. See `docs/Implementierungsplan.md`, chapter 4.5.

mod access;
mod schema;
mod users;

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::cube::{Cube, OpRecord, OpSink, WriteContext};
use crate::dimension::{Dimension, DimensionData};
use crate::exchange::{ImportReport, ImportSpec, SourceKind};
use crate::{Error, MemberId, Result};

pub use access::{Access, CubeRights, Grant};
pub use schema::SCHEMA_VERSION;
pub use users::{MIN_PASSWORD_LEN, Role, User, random_bytes_16};

/// Default retention of journal entries.
pub const DEFAULT_JOURNAL_DAYS: i64 = 90;

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Open database: connection, lock file and the loaded model.
pub struct Database {
    path: PathBuf,
    conn: Arc<Mutex<Connection>>,
    _lock: File,
    dims: BTreeMap<String, Arc<Dimension>>,
    cubes: BTreeMap<String, Cube>,
    journal_days: i64,
}

/// One journal line (who changed what when).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JournalEntry {
    pub id: i64,
    pub ts: i64,
    pub user: String,
    pub op: String,
    pub cube: String,
    pub generation: i64,
    pub n_cells: i64,
    pub detail: String,
}

/// Cells of one journal entry: address, old and new value.
pub type JournalCell = (Vec<MemberId>, Option<f64>, Option<f64>);

/// Cells dropped from a cube by a dimension change: cube, dimension index, `(address, value)`.
type Dropped = (String, usize, Vec<(Vec<MemberId>, f64)>);

/// Selection for [`Database::journal`].
#[derive(Debug, Clone, Default)]
pub struct JournalQuery {
    pub cube: Option<String>,
    pub user: Option<String>,
    pub since: Option<i64>,
    pub limit: Option<usize>,
}

#[derive(Serialize, Deserialize)]
struct CubeDims(Vec<String>);

/// Packs base-member ids of one cell into a key (little-endian u32 per dimension).
pub(crate) fn encode_key(addr: &[MemberId]) -> Vec<u8> {
    addr.iter().flat_map(|m| m.to_le_bytes()).collect()
}

pub(crate) fn decode_key(k: &[u8]) -> Result<Vec<MemberId>> {
    if !k.len().is_multiple_of(4) {
        return Err(Error::Storage("a fact key has an invalid length".into()));
    }
    Ok(k.as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

/// Date and time for file names, `20261103-201500` (UTC).
pub fn timestamp_label() -> String {
    let secs = now();
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn lock_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".lock");
    PathBuf::from(p)
}

/// The sink given to each cube: persists an operation in one transaction.
struct SqliteSink {
    conn: Arc<Mutex<Connection>>,
}

impl OpSink for SqliteSink {
    fn record(&mut self, cube: &str, op: &OpRecord) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for c in &op.changes {
            let k = encode_key(&c.addr);
            match c.new {
                Some(v) => tx.execute(
                    "INSERT INTO facts (cube, k, v) VALUES (?1, ?2, ?3) ON CONFLICT (cube, k) DO UPDATE SET v = excluded.v",
                    params![cube, k, v],
                )?,
                None => tx.execute("DELETE FROM facts WHERE cube = ?1 AND k = ?2", params![cube, k])?,
            };
        }
        let journal_id: i64 = tx.query_row(
            "INSERT INTO journal (ts, user, op, cube, generation, n_cells) VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING id",
            params![now(), op.user, op.kind, cube, op.generation as i64, op.changes.len() as i64],
            |r| r.get(0),
        )?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO journal_cells (op_id, k, old, new) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for c in &op.changes {
                stmt.execute(params![journal_id, encode_key(&c.addr), c.old, c.new])?;
            }
        }
        tx.execute(
            "UPDATE cubes SET generation = ?2, next_op = ?3 WHERE name = ?1",
            params![cube, op.generation as i64, op.id as i64 + 1],
        )?;
        tx.commit()?;
        Ok(())
    }
}

impl Database {
    /// Creates a new, empty database; fails if the file exists.
    pub fn create(path: impl AsRef<Path>) -> Result<Database> {
        if path.as_ref().exists() {
            return Err(Error::Storage(format!(
                "'{}' already exists",
                path.as_ref().display()
            )));
        }
        Self::open_inner(path.as_ref(), true)
    }

    /// Opens an existing database exclusively and migrates its schema if needed.
    pub fn open(path: impl AsRef<Path>) -> Result<Database> {
        if !path.as_ref().exists() {
            return Err(Error::NotFound {
                kind: "database",
                name: path.as_ref().display().to_string(),
            });
        }
        Self::open_inner(path.as_ref(), false)
    }

    fn open_inner(path: &Path, create: bool) -> Result<Database> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path(path))?;
        lock.try_lock()
            .map_err(|_| Error::Locked(path.display().to_string()))?;
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        if !create && schema::version(&conn)? == 0 {
            return Err(Error::Storage(format!(
                "'{}' is not a NeOlap database",
                path.display()
            )));
        }
        if !create {
            // Safety net: copy the file before an upgrade changes its schema.
            let found = schema::version(&conn)?;
            if found < SCHEMA_VERSION {
                let mut backup = path.as_os_str().to_owned();
                backup.push(format!(".before-v{SCHEMA_VERSION}"));
                conn.execute("VACUUM INTO ?1", params![backup.to_string_lossy()])?;
            }
        }
        schema::migrate(&mut conn)?;
        let mut db = Database {
            path: path.to_path_buf(),
            conn: Arc::new(Mutex::new(conn)),
            _lock: lock,
            dims: BTreeMap::new(),
            cubes: BTreeMap::new(),
            journal_days: DEFAULT_JOURNAL_DAYS,
        };
        db.load()?;
        db.prune_journal()?;
        Ok(db)
    }

    fn load(&mut self) -> Result<()> {
        let conn = self.conn.lock();
        let mut dims = BTreeMap::new();
        {
            let mut stmt = conn.prepare("SELECT name, data FROM dimensions ORDER BY name")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (name, data) = row?;
                let data: DimensionData = serde_json::from_str(&data).map_err(|e| {
                    Error::Storage(format!("dimension '{name}' is unreadable: {e}"))
                })?;
                dims.insert(name, Arc::new(Dimension::from_data(data)?));
            }
        }
        let mut cubes = BTreeMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT name, dims, rules, generation, next_op FROM cubes ORDER BY name",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })?;
            for row in rows {
                let (name, dim_names, rules, generation, _next_op) = row?;
                let CubeDims(dim_names) = CubeDims(
                    serde_json::from_str(&dim_names)
                        .map_err(|e| Error::Storage(format!("cube '{name}': {e}")))?,
                );
                let cube_dims: Vec<Arc<Dimension>> = dim_names
                    .iter()
                    .map(|d| {
                        dims.get(d).cloned().ok_or_else(|| {
                            Error::Storage(format!(
                                "cube '{name}' refers to the missing dimension '{d}'"
                            ))
                        })
                    })
                    .collect::<Result<_>>()?;
                let mut cube = Cube::new(name.clone(), cube_dims);
                let mut facts = conn.prepare("SELECT k, v FROM facts WHERE cube = ?1")?;
                let cells = facts
                    .query_map(params![name], |r| {
                        Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, f64>(1)?))
                    })?
                    .map(|row| {
                        let (k, v) = row?;
                        Ok((decode_key(&k)?, v))
                    })
                    .collect::<Result<Vec<_>>>()?;
                cube.load_cells(cells, generation as u64)?;
                if !rules.is_empty() {
                    cube.set_rules(&rules)?;
                    cube.restore_generation(generation as u64);
                }
                cube.set_sink(Some(Box::new(SqliteSink {
                    conn: self.conn.clone(),
                })));
                cubes.insert(name, cube);
            }
        }
        drop(conn);
        self.dims = dims;
        self.cubes = cubes;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn schema_version(&self) -> Result<i64> {
        schema::version(&self.conn.lock())
    }

    pub fn dimension(&self, name: &str) -> Option<&Arc<Dimension>> {
        self.dims
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, d)| d)
    }

    pub fn dimension_names(&self) -> impl Iterator<Item = &str> {
        self.dims.keys().map(String::as_str)
    }

    pub fn cube(&self, name: &str) -> Option<&Cube> {
        self.cubes
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, c)| c)
    }

    pub fn cube_mut(&mut self, name: &str) -> Option<&mut Cube> {
        self.cubes
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, c)| c)
    }

    pub fn cubes(&self) -> impl Iterator<Item = &Cube> {
        self.cubes.values()
    }

    pub(crate) fn tx<T>(&self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    fn log(
        tx: &Transaction,
        user: &str,
        op: &str,
        cube: &str,
        generation: u64,
        n: usize,
        detail: &str,
    ) -> Result<()> {
        tx.execute(
            "INSERT INTO journal (ts, user, op, cube, generation, n_cells, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![now(), user, op, cube, generation as i64, n as i64, detail],
        )?;
        Ok(())
    }

    /// Stores a new or changed dimension. Cubes that use it are updated: facts of removed members
    /// (or of members that became aggregates) are dropped and journaled.
    pub fn save_dimension(&mut self, dim: Dimension, user: &str) -> Result<()> {
        let name = dim.name().to_string();
        let data =
            serde_json::to_string(&dim.to_data()).map_err(|e| Error::Storage(e.to_string()))?;
        let dim = Arc::new(dim);
        let users: Vec<String> = self
            .cubes
            .iter()
            .filter(|(_, c)| c.dimensions().iter().any(|d| d.name() == name))
            .map(|(n, _)| n.clone())
            .collect();
        // Work out the new cube states first; commit to memory only after the file is written.
        let mut staged: Vec<Dropped> = Vec::new();
        for cube_name in &users {
            let cube = &self.cubes[cube_name];
            for i in cube
                .dimensions()
                .iter()
                .enumerate()
                .filter(|(_, d)| d.name() == name)
                .map(|(i, _)| i)
            {
                let dropped: Vec<(Vec<MemberId>, f64)> = cube
                    .export_cells()
                    .into_iter()
                    .filter(|(addr, _)| !dim.is_leaf(addr[i]))
                    .collect();
                staged.push((cube_name.clone(), i, dropped));
            }
        }
        self.tx(|tx| {
            tx.execute("INSERT INTO dimensions (name, data) VALUES (?1, ?2) ON CONFLICT (name) DO UPDATE SET data = excluded.data", params![name, data])?;
            for (cube_name, _, dropped) in &staged {
                let generation = self.cubes[cube_name].generation() + 1;
                for (addr, _) in dropped {
                    tx.execute("DELETE FROM facts WHERE cube = ?1 AND k = ?2", params![cube_name, encode_key(addr)])?;
                }
                tx.execute("UPDATE cubes SET generation = ?2 WHERE name = ?1", params![cube_name, generation as i64])?;
                Self::log(tx, user, "dimension", cube_name, generation, dropped.len(), &format!("dimension '{name}' changed, {} cells dropped", dropped.len()))?;
            }
            if staged.is_empty() {
                Self::log(tx, user, "dimension", "", 0, 0, &format!("dimension '{name}' saved"))?;
            }
            Ok(())
        })?;
        for (cube_name, i, _) in staged {
            if let Some(cube) = self.cubes.get_mut(&cube_name) {
                cube.replace_dimension(i, dim.clone())?;
            }
        }
        self.dims.insert(name, dim);
        Ok(())
    }

    /// Creates a cube over existing dimensions.
    pub fn create_cube(&mut self, name: &str, dimension_names: &[&str], user: &str) -> Result<()> {
        if self.cube(name).is_some() {
            return Err(Error::Conflict(format!("cube '{name}' already exists")));
        }
        let dims: Vec<Arc<Dimension>> = dimension_names
            .iter()
            .map(|d| {
                self.dimension(d).cloned().ok_or_else(|| Error::NotFound {
                    kind: "dimension",
                    name: (*d).to_string(),
                })
            })
            .collect::<Result<_>>()?;
        let names: Vec<String> = dims.iter().map(|d| d.name().to_string()).collect();
        let json = serde_json::to_string(&names).map_err(|e| Error::Storage(e.to_string()))?;
        self.tx(|tx| {
            tx.execute(
                "INSERT INTO cubes (name, dims) VALUES (?1, ?2)",
                params![name, json],
            )?;
            Self::log(tx, user, "create_cube", name, 0, 0, &names.join(", "))
        })?;
        let mut cube = Cube::new(name, dims);
        cube.set_sink(Some(Box::new(SqliteSink {
            conn: self.conn.clone(),
        })));
        self.cubes.insert(name.to_string(), cube);
        Ok(())
    }

    /// Replaces the rules of a cube (syntax errors reject the text and change nothing).
    pub fn set_rules(
        &mut self,
        cube: &str,
        text: &str,
        user: &str,
    ) -> Result<Vec<crate::rules::RuleStatus>> {
        let key = self
            .cube(cube)
            .map(|c| c.name().to_string())
            .ok_or_else(|| Error::NotFound {
                kind: "cube",
                name: cube.to_string(),
            })?;
        crate::rules::RuleSet::build(text, self.cubes[&key].dimensions())?;
        let generation = self.cubes[&key].generation() + 1;
        self.tx(|tx| {
            tx.execute(
                "UPDATE cubes SET rules = ?2, generation = ?3 WHERE name = ?1",
                params![key, text, generation as i64],
            )?;
            Self::log(tx, user, "rules", &key, generation, 0, "")
        })?;
        self.cubes.get_mut(&key).expect("checked").set_rules(text)
    }

    /// Journal entries, newest first.
    pub fn journal(&self, q: &JournalQuery) -> Result<Vec<JournalEntry>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, ts, user, op, cube, generation, n_cells, detail FROM journal
             WHERE (?1 IS NULL OR cube = ?1) AND (?2 IS NULL OR user = ?2) AND (?3 IS NULL OR ts >= ?3)
             ORDER BY id DESC LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![q.cube, q.user, q.since, q.limit.map_or(-1, |l| l as i64)],
            |r| {
                Ok(JournalEntry {
                    id: r.get(0)?,
                    ts: r.get(1)?,
                    user: r.get(2)?,
                    op: r.get(3)?,
                    cube: r.get(4)?,
                    generation: r.get(5)?,
                    n_cells: r.get(6)?,
                    detail: r.get(7)?,
                })
            },
        )?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Old and new value of the cells of one journal entry.
    pub fn journal_cells(&self, journal_id: i64) -> Result<Vec<JournalCell>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT k, old, new FROM journal_cells WHERE op_id = ?1")?;
        let rows = stmt.query_map(params![journal_id], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, Option<f64>>(1)?,
                r.get::<_, Option<f64>>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (k, o, n) = row?;
            Ok((decode_key(&k)?, o, n))
        })
        .collect()
    }

    /// Sets the retention and deletes older journal entries.
    pub fn set_journal_retention(&mut self, days: i64) -> Result<usize> {
        self.journal_days = days;
        self.prune_journal()
    }

    fn prune_journal(&self) -> Result<usize> {
        let cutoff = now() - self.journal_days * 86_400;
        self.tx(|tx| {
            tx.execute(
                "DELETE FROM journal_cells WHERE op_id IN (SELECT id FROM journal WHERE ts < ?1)",
                params![cutoff],
            )?;
            Ok(tx.execute("DELETE FROM journal WHERE ts < ?1", params![cutoff])?)
        })
    }

    /// Consistent online copy of the database (SQLite backup API); the source stays usable.
    pub fn backup(&self, target: impl AsRef<Path>) -> Result<()> {
        let target = target.as_ref();
        if target.exists() {
            return Err(Error::Storage(format!(
                "'{}' already exists",
                target.display()
            )));
        }
        let conn = self.conn.lock();
        let mut dst = Connection::open(target)?;
        let backup = rusqlite::backup::Backup::new(&conn, &mut dst)?;
        backup.run_to_completion(256, std::time::Duration::from_millis(0), None)?;
        Ok(())
    }

    /// Stores an import specification in the model (replaces one with the same name).
    pub fn save_import_spec(&mut self, toml_text: &str, user: &str) -> Result<()> {
        let spec = ImportSpec::from_toml(toml_text)?;
        let cube = self
            .cube(&spec.cube)
            .map(|c| c.name().to_string())
            .ok_or_else(|| Error::NotFound {
                kind: "cube",
                name: spec.cube.clone(),
            })?;
        self.tx(|tx| {
            tx.execute(
                "INSERT INTO import_specs (name, cube, spec) VALUES (?1, ?2, ?3) ON CONFLICT (name) DO UPDATE SET cube = excluded.cube, spec = excluded.spec",
                params![spec.name, cube, toml_text],
            )?;
            Self::log(tx, user, "import_spec", &cube, 0, 0, &spec.name)
        })
    }

    /// Names of the stored import specifications.
    pub fn import_spec_names(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT name FROM import_specs ORDER BY name")?;
        Ok(stmt
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?)
    }

    /// Text of a stored import specification.
    pub fn import_spec(&self, name: &str) -> Result<String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT spec FROM import_specs WHERE name = ?1 COLLATE NOCASE",
            params![name],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| Error::NotFound {
            kind: "import specification",
            name: name.to_string(),
        })
    }

    /// Runs an import (see [`crate::exchange::run_import`]); unknown members are created through
    /// [`save_dimension`](Self::save_dimension) so that they reach the file. A real run is
    /// written to the journal as one operation and can be undone.
    pub fn run_import(
        &mut self,
        spec: &ImportSpec,
        kind: SourceKind,
        data: &[u8],
        ctx: &WriteContext,
        dry_run: bool,
    ) -> Result<ImportReport> {
        let key = self
            .cube(&spec.cube)
            .map(|c| c.name().to_string())
            .ok_or_else(|| Error::NotFound {
                kind: "cube",
                name: spec.cube.clone(),
            })?;
        // Unknown members are created *before* any cell is written, and the dimension reaches the
        // file first: a crash in between leaves a valid file (only the new members exist).
        let mut cube = self.cubes.remove(&key).expect("checked");
        let conn = self.conn.clone();
        let user = ctx.user.clone();
        let mut changed: Vec<Arc<Dimension>> = Vec::new();
        let result = {
            let mut create = |c: &mut Cube,
                              d: usize,
                              members: Vec<(String, String)>|
             -> Result<()> {
                let dim = c.dimensions()[d].clone();
                let mut ed = dim.edit();
                for (name, parent) in &members {
                    let p = ed.get_or_add(parent)?;
                    let m = ed.add(name)?;
                    ed.add_edge(p, m, 1.0)?;
                }
                let new = Arc::new(ed.commit()?);
                let data = serde_json::to_string(&new.to_data())
                    .map_err(|e| Error::Storage(e.to_string()))?;
                {
                    let mut conn = conn.lock();
                    let tx = conn.transaction()?;
                    tx.execute(
                        "INSERT INTO dimensions (name, data) VALUES (?1, ?2) ON CONFLICT (name) DO UPDATE SET data = excluded.data",
                        params![new.name(), data],
                    )?;
                    Self::log(
                        &tx,
                        &user,
                        "dimension",
                        c.name(),
                        c.generation(),
                        0,
                        &format!(
                            "{} member(s) added by import '{}'",
                            members.len(),
                            spec.name
                        ),
                    )?;
                    tx.commit()?;
                }
                // no cell is affected: the parents were checked to be aggregates or empty
                c.replace_dimension(d, new.clone())?;
                changed.push(new);
                Ok(())
            };
            crate::exchange::run_import(
                &mut cube,
                spec,
                kind,
                data,
                ctx,
                dry_run,
                Some(&mut create),
            )
        };
        self.cubes.insert(key.clone(), cube);
        for dim in changed {
            let name = dim.name().to_string();
            let others: Vec<String> = self
                .cubes
                .iter()
                .filter(|(n, c)| **n != key && c.dimensions().iter().any(|x| x.name() == name))
                .map(|(n, _)| n.clone())
                .collect();
            for other in others {
                let idx = self.cubes[&other]
                    .dimensions()
                    .iter()
                    .position(|x| x.name() == name)
                    .expect("uses it");
                self.cubes
                    .get_mut(&other)
                    .expect("exists")
                    .replace_dimension(idx, dim.clone())?;
            }
            self.dims.insert(name, dim);
        }
        result
    }

    /// Number of rows in `facts` (for checks).
    pub fn stored_cells(&self, cube: &str) -> Result<i64> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT COUNT(*) FROM facts WHERE cube = ?1",
                params![cube],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }
}

#[cfg(test)]
mod tests;
