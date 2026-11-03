//! `neolap` – command line interface.
//!
//! Milestone M0: `neolap version` and `neolap serve`. Model, import and query
//! commands follow in milestone M7 (see docs/Implementierungsplan.md, chapter 8).

use std::net::SocketAddr;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "neolap",
    version,
    about = "NeOlap – lightweight multidimensional database"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print version information.
    Version {
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Start the server with web UI.
    Serve {
        /// Address to listen on. Default: localhost only.
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: SocketAddr,
    },
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Version { json } => {
            let info = neolap_core::build_info();
            if json {
                println!("{}", serde_json::to_string(&info).expect("serializable"));
            } else {
                println!("{} {}", info.name, info.version);
            }
            std::process::ExitCode::SUCCESS
        }
        Command::Serve { listen } => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "info".into()),
                )
                .init();
            let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
            match runtime.block_on(neolap_server::serve(listen)) {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("error: {err}");
                    std::process::ExitCode::FAILURE
                }
            }
        }
    }
}
