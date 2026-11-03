//! NeOlap core engine.
//!
//! This crate will contain the complete domain logic of NeOlap: dimensions and
//! hierarchies, the fact store, aggregation, write-back with splashing, the rule
//! language, persistence, import/export and access control.
//! See `docs/Implementierungsplan.md` for the roadmap (milestones M1–M6).
//!
//! Milestone M0 provides the basic vocabulary shared by all front ends, M1 the dimension model
//! (see [`dimension`]).

use serde::Serialize;

pub mod cube;
pub mod dimension;
pub mod persist;
pub mod rules;

pub use cube::{
    CellError, CellValue, Cube, CubeProvider, FactStore, Grid, Input, WriteContext, WriteResult,
};

pub use dimension::{
    AttrValue, Dimension, DimensionEditor, DimensionKind, LeafSet, LeafShape, Subset,
};

/// Crate version, shared by CLI, server and Python module.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Identifier of a member within one dimension (index into the member table).
pub type MemberId = u32;

/// Common error type of the NeOlap engine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A referenced object (dimension, member, cube, …) does not exist.
    #[error("{kind} '{name}' not found")]
    NotFound { kind: &'static str, name: String },
    /// The operation is not allowed in the current state or for the current user.
    #[error("invalid operation: {0}")]
    InvalidOperation(String),
    /// The hierarchy of a dimension contains a cycle.
    #[error("cycle in hierarchy of dimension '{dimension}' involving '{member}'")]
    Cycle { dimension: String, member: String },
    /// Name, alias or path-weight conflict (e.g. a leaf reachable with contradicting weights).
    #[error("conflict: {0}")]
    Conflict(String),
    /// A cell in the written area was changed by someone else since the client's view.
    #[error("cell [{cell}] was changed by '{user}' in the meantime (now {})", fmt_value(*.value))]
    WriteConflict {
        user: String,
        cell: String,
        value: Option<f64>,
    },
    /// The user may not write a cell of the affected area.
    #[error("access denied: {0}")]
    Denied(String),
    /// The database file could not be read or written.
    #[error("storage error: {0}")]
    Storage(String),
    /// Another process has the database open exclusively.
    #[error("database '{0}' is in use by another process")]
    Locked(String),
    /// Input (CSV, specification, …) could not be parsed.
    #[error("parse error: {0}")]
    Parse(String),
}

fn fmt_value(v: Option<f64>) -> String {
    v.map_or_else(|| "empty".to_string(), |v| v.to_string())
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Storage(e.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Storage(e.to_string())
    }
}

/// Convenience result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Basic information about the engine build, used by `neolap version` and `/api/v1/health`.
#[derive(Debug, Clone, Serialize)]
pub struct BuildInfo {
    pub name: &'static str,
    pub version: &'static str,
}

/// Returns the build information of this engine.
pub fn build_info() -> BuildInfo {
    BuildInfo {
        name: "NeOlap",
        version: VERSION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_info_reports_crate_version() {
        let info = build_info();
        assert_eq!(info.name, "NeOlap");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn not_found_error_is_readable() {
        let err = Error::NotFound {
            kind: "dimension",
            name: "regions".into(),
        };
        assert_eq!(err.to_string(), "dimension 'regions' not found");
    }
}
