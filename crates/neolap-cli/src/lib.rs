//! `neolap` command line: model, data and user administration on a database file.
//!
//! The commands run on the file directly (exclusive, see `Database::open`). Everything is in
//! this library so that the end-to-end tests call [`run`] without starting a process; the binary
//! in `main.rs` only adds `serve`, `healthcheck` and the exit code.

mod commands;
mod output;

pub use commands::{Cli, Command, DataCommand, run};
pub use output::{Output, V};
