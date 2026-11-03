//! NeOlap core engine.
//!
//! This crate will contain the complete domain logic of NeOlap: dimensions and
//! hierarchies, the fact store, aggregation, write-back with splashing, the rule
//! language, persistence, import/export and access control.
//! See `docs/Implementierungsplan.md` for the roadmap (milestones M1–M6).
//!
//! Milestone M0 only provides the basic vocabulary shared by all front ends.

use serde::Serialize;

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
