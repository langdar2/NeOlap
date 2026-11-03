//! Data exchange: import specifications, CSV/xlsx import with `replace_area`, export.
//! See `docs/Implementierungsplan.md`, chapter 4.6.

mod export;
mod import;
mod spec;
mod table;

pub use export::{Cell, ExportSpec, ExportTable, Layout, export_table, to_csv, to_xlsx};
pub use import::{ImportReport, RejectedRow, run_import, run_import_file};
pub use spec::{ImportSpec, LoadMode, MapEntry, Options, SourceSpec, UnknownPolicy};
pub use table::SourceKind;

#[cfg(test)]
mod tests;
