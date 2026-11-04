//! `neolap` – command line interface.
//!
//! Data, model and user commands live in the library (`neolap_cli`); this binary adds
//! `serve` and `healthcheck`, which need the server runtime, and maps errors to exit codes.
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

use clap::Parser;
use neolap_cli::{Cli, Command, DataCommand, Output};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Data(DataCommand::Serve {
            listen,
            data_dir,
            database,
            secure_cookies,
            trust_proxy,
            admin_user,
            admin_password,
            backup_interval_hours,
            backup_keep,
        }) => serve(
            listen,
            data_dir,
            database,
            secure_cookies,
            trust_proxy,
            match initial_admin(admin_user, admin_password) {
                Ok(a) => a,
                Err(msg) => {
                    eprintln!("error: {msg}");
                    return ExitCode::FAILURE;
                }
            },
            // (interval, keep): 0 hours switches the scheduled backup off
            (
                (backup_interval_hours > 0)
                    .then(|| Duration::from_secs(backup_interval_hours.saturating_mul(3600))),
                backup_keep,
            ),
        ),
        Command::Data(DataCommand::Healthcheck { addr, timeout }) => {
            match healthcheck(&addr, Duration::from_secs(timeout)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(msg) => {
                    eprintln!("unhealthy: {msg}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
            let mut output = Output {
                out: &mut out,
                err: &mut err,
                json: cli.json,
            };
            match neolap_cli::run(cli, &mut output) {
                Ok(()) => ExitCode::SUCCESS,
                Err(msg) => {
                    eprintln!("error: {msg}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

/// The first administrator from the two settings. Empty values count as not set (that is what
/// Docker Compose passes for an unset `${VAR:-}`); one of the two alone is a mistake worth stopping
/// for, not something to silently start without an administrator.
fn initial_admin(
    user: Option<String>,
    password: Option<String>,
) -> Result<Option<(String, String)>, String> {
    let user = user.filter(|u| !u.is_empty());
    let password = password.filter(|p| !p.is_empty());
    match (user, password) {
        (Some(u), Some(p)) => Ok(Some((u, p))),
        (None, None) => Ok(None),
        (Some(_), None) => Err("NEOLAP_ADMIN_USER is set but NEOLAP_ADMIN_PASSWORD is not".into()),
        (None, Some(_)) => Err("NEOLAP_ADMIN_PASSWORD is set but NEOLAP_ADMIN_USER is not".into()),
    }
}

fn serve(
    listen: SocketAddr,
    data_dir: PathBuf,
    database: String,
    secure_cookies: bool,
    trust_proxy: bool,
    admin: Option<(String, String)>,
    backup: (Option<Duration>, usize),
) -> ExitCode {
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
    let mut config = neolap_server::ServerConfig::new(listen, data_dir);
    config.database = database;
    config.insecure_cookies = !secure_cookies;
    config.trust_proxy = trust_proxy;
    config.initial_admin = admin;
    (config.backup_interval, config.backup_keep) = backup;
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
