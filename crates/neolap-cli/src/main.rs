//! `neolap` – command line interface.
//!
//! Milestone M0: `neolap version`, `neolap serve`, `neolap healthcheck`.
//! Model, import and query commands follow in milestone M7
//! (see docs/Implementierungsplan.md, chapter 8).
//!
//! All `serve` options can also be set via environment variables (`NEOLAP_*`),
//! which is how the Docker image is configured.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    path::PathBuf,
    process::ExitCode,
    time::Duration,
};

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
        #[arg(long, env = "NEOLAP_LISTEN", default_value = "127.0.0.1:8080")]
        listen: SocketAddr,
        /// Directory for databases, journal and backups.
        #[arg(long, env = "NEOLAP_DATA_DIR", default_value = "./data")]
        data_dir: PathBuf,
    },
    /// Check whether a running server is healthy (exit code 0 = healthy).
    /// Used as Docker HEALTHCHECK, since the runtime image has no shell or curl.
    Healthcheck {
        /// Server address to check.
        #[arg(
            long,
            env = "NEOLAP_HEALTHCHECK_ADDR",
            default_value = "127.0.0.1:8080"
        )]
        addr: String,
        /// Timeout in seconds.
        #[arg(long, default_value_t = 3)]
        timeout: u64,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Version { json } => {
            let info = neolap_core::build_info();
            if json {
                println!("{}", serde_json::to_string(&info).expect("serializable"));
            } else {
                println!("{} {}", info.name, info.version);
            }
            ExitCode::SUCCESS
        }
        Command::Serve { listen, data_dir } => serve(listen, data_dir),
        Command::Healthcheck { addr, timeout } => {
            match healthcheck(&addr, Duration::from_secs(timeout)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(msg) => {
                    eprintln!("unhealthy: {msg}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn serve(listen: SocketAddr, data_dir: PathBuf) -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    if let Err(err) = std::fs::create_dir_all(&data_dir) {
        eprintln!(
            "error: cannot create data directory {}: {err}",
            data_dir.display()
        );
        return ExitCode::FAILURE;
    }
    let config = neolap_server::ServerConfig { listen, data_dir };
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    match runtime.block_on(neolap_server::serve(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Minimal HTTP/1.1 GET of `/api/v1/health` without external dependencies.
fn healthcheck(addr: &str, timeout: Duration) -> Result<(), String> {
    let socket = addr
        .to_socket_addrs()
        .map_err(|e| format!("invalid address {addr}: {e}"))?
        .next()
        .ok_or_else(|| format!("address {addr} did not resolve"))?;
    let mut stream =
        TcpStream::connect_timeout(&socket, timeout).map_err(|e| format!("connect: {e}"))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    write!(
        stream,
        "GET /api/v1/health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| format!("send: {e}"))?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("read: {e}"))?;
    let status_line = response.lines().next().unwrap_or_default();
    if !status_line.contains(" 200 ") {
        return Err(format!("unexpected status: {status_line}"));
    }
    if !response.contains("\"status\":\"ok\"") {
        return Err("health endpoint did not report ok".into());
    }
    Ok(())
}
