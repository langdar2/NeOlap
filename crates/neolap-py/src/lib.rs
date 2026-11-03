//! Python module `neolap`.
//!
//! Milestone M0 exposes only the version. Local and remote database access
//! follows in milestone M10 (see docs/Implementierungsplan.md, chapter 9).

use pyo3::prelude::*;

/// Returns the NeOlap version string.
#[pyfunction]
fn version() -> &'static str {
    neolap_core::VERSION
}

#[pymodule]
fn neolap(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", neolap_core::VERSION)?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    Ok(())
}
