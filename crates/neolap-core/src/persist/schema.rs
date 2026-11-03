//! SQLite schema and migrations.

use rusqlite::{Connection, params};

use crate::{Error, Result};

/// Version written by this build; older files are migrated at open, newer ones are rejected.
pub const SCHEMA_VERSION: i64 = 3;

/// Each entry migrates from version `index` to `index + 1`.
const MIGRATIONS: &[&str] = &["
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE dimensions (name TEXT PRIMARY KEY, data TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE cubes (
    name TEXT PRIMARY KEY,
    dims TEXT NOT NULL,
    rules TEXT NOT NULL DEFAULT '',
    generation INTEGER NOT NULL DEFAULT 0,
    next_op INTEGER NOT NULL DEFAULT 1
) WITHOUT ROWID;
CREATE TABLE facts (cube TEXT NOT NULL, k BLOB NOT NULL, v REAL NOT NULL, PRIMARY KEY (cube, k)) WITHOUT ROWID;
CREATE TABLE journal (
    id INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    user TEXT NOT NULL,
    op TEXT NOT NULL,
    cube TEXT NOT NULL,
    generation INTEGER NOT NULL,
    n_cells INTEGER NOT NULL,
    detail TEXT NOT NULL DEFAULT ''
);
CREATE INDEX journal_ts ON journal (ts);
CREATE TABLE journal_cells (op_id INTEGER NOT NULL, k BLOB NOT NULL, old REAL, new REAL);
CREATE INDEX journal_cells_op ON journal_cells (op_id);
CREATE TABLE users (name TEXT PRIMARY KEY, password_hash TEXT NOT NULL, role TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE grants (user TEXT NOT NULL, cube TEXT NOT NULL, access TEXT NOT NULL, area TEXT NOT NULL DEFAULT '', PRIMARY KEY (user, cube, area)) WITHOUT ROWID;
", "
CREATE TABLE import_specs (name TEXT PRIMARY KEY, cube TEXT NOT NULL, spec TEXT NOT NULL) WITHOUT ROWID;
", "
CREATE TABLE tokens (id TEXT PRIMARY KEY, user TEXT NOT NULL, label TEXT NOT NULL DEFAULT '', token_hash TEXT NOT NULL, created INTEGER NOT NULL) WITHOUT ROWID;
CREATE INDEX tokens_user ON tokens (user);
"];

pub fn version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Brings the file to [`SCHEMA_VERSION`]. Returns the version found before.
pub fn migrate(conn: &mut Connection) -> Result<i64> {
    let found = version(conn)?;
    if found > SCHEMA_VERSION {
        return Err(Error::Storage(format!(
            "the file has schema version {found}, this build understands up to {SCHEMA_VERSION}; please update NeOlap"
        )));
    }
    for v in found..SCHEMA_VERSION {
        let tx = conn.transaction()?;
        tx.execute_batch(MIGRATIONS[v as usize])?;
        tx.pragma_update(None, "user_version", v + 1)?;
        tx.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?1)",
            params![(v + 1).to_string()],
        )?;
        tx.commit()?;
    }
    Ok(found)
}
