//! NeOlap server: REST API (`/api/v1/...`) and the embedded web UI.
//!
//! Milestone M0 provides the router skeleton, a health endpoint and static
//! delivery of the web UI that is compiled into the binary.

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    body::Body,
    extract::DefaultBodyLimit,
    http::{StatusCode, Uri, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use rust_embed::RustEmbed;
use serde::Serialize;
use tower_http::trace::TraceLayer;

mod api;
mod auth;
mod error;
mod jobs;
mod state;

pub use state::AppState;

/// Helpers for tests of this crate and of clients; not part of the stable API.
#[doc(hidden)]
pub mod testing {
    use crate::state::Event;

    /// A cell-change event, for driving the event stream in tests.
    pub fn event(cube: &str, generation: u64) -> Event {
        Event {
            kind: "cells",
            cube: Some(cube.to_string()),
            generation,
            user: "test".into(),
            cells_changed: 1,
        }
    }
}

/// Web UI files from `<repo>/ui`, embedded at compile time. The tests of the UI (`ui/tests/`) stay
/// out of the binary: they are not part of the application and must not be served.
#[derive(RustEmbed)]
#[folder = "../../ui/"]
#[exclude = "tests/*"]
struct UiAssets;

#[derive(Serialize)]
struct Health {
    status: &'static str,
    name: &'static str,
    version: &'static str,
}

async fn health() -> Json<Health> {
    let info = neolap_core::build_info();
    Json(Health {
        status: "ok",
        name: info.name,
        version: info.version,
    })
}

async fn static_file(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match UiAssets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.as_ref().to_string())],
                Body::from(file.data.into_owned()),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn api_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "unknown API endpoint" })),
    )
        .into_response()
}

/// Builds the HTTP router (API + UI) on top of the shared state.
pub fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/login", post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/me", get(auth::me))
        .route("/session", get(auth::session))
        .route("/model", get(api::model))
        .route("/dimensions/{d}/members", get(api::members))
        .route("/dimensions/{d}", patch(api::patch_dimension))
        .route("/cubes/{c}/view", post(api::view))
        .route("/cubes/{c}/cell", get(api::cell))
        .route("/cubes/{c}/write", post(api::write))
        .route("/cubes/{c}/undo", post(api::undo))
        .route("/cubes/{c}/rules", get(api::get_rules).put(api::put_rules))
        .route("/cubes/{c}/export", get(jobs::export))
        .route(
            "/imports",
            post(jobs::start_import).layer(DefaultBodyLimit::max(jobs::MAX_UPLOAD + 1024 * 1024)),
        )
        .route("/jobs/{id}", get(jobs::job))
        .route("/journal", get(api::journal))
        .route("/events", get(jobs::events))
        .route("/admin/users", get(api::list_users).post(api::add_user))
        .route(
            "/admin/users/{name}",
            patch(api::patch_user).delete(api::delete_user),
        )
        .route("/admin/users/{name}/token", post(api::create_token))
        .route(
            "/admin/grants",
            get(api::list_grants)
                .put(api::put_grant)
                .delete(api::delete_grant),
        )
        .route("/admin/backup", post(api::backup))
        .fallback(api_not_found)
        .with_state(state);
    Router::new()
        .nest("/api/v1", api)
        .fallback(static_file)
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(middleware::from_fn(auth::security_headers))
        .layer(TraceLayer::new_for_http())
}

/// Server configuration.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Socket address to listen on.
    pub listen: SocketAddr,
    /// Directory for databases, journal and backups (a Docker volume in container setups).
    pub data_dir: PathBuf,
    /// File name of the database inside `data_dir`.
    pub database: String,
    /// Plain-HTTP cookies (no `Secure` flag): for localhost and tests. Behind TLS set `false`.
    pub insecure_cookies: bool,
    /// Trust `X-Forwarded-For` (behind a reverse proxy such as the Caddy profile).
    pub trust_proxy: bool,
    /// First administrator, created at start if the database has no user yet.
    pub initial_admin: Option<(String, String)>,
}

impl ServerConfig {
    pub fn new(listen: SocketAddr, data_dir: PathBuf) -> Self {
        ServerConfig {
            listen,
            data_dir,
            database: "neolap.neolap".into(),
            insecure_cookies: true,
            trust_proxy: false,
            initial_admin: None,
        }
    }
}

/// Opens (or creates) the database of the server.
pub fn open_state(config: &ServerConfig) -> Result<Arc<AppState>, neolap_core::Error> {
    std::fs::create_dir_all(&config.data_dir)?;
    let path = config.data_dir.join(&config.database);
    let mut db = if path.exists() {
        neolap_core::Database::open(&path)?
    } else {
        neolap_core::Database::create(&path)?
    };
    if db.users()?.is_empty() {
        match &config.initial_admin {
            Some((name, password)) => {
                db.add_user(name, password, neolap_core::persist::Role::Admin, "setup")?;
                tracing::info!("created the first administrator '{name}'");
            }
            None => tracing::warn!(
                "the database has no users, so nobody can log in. Set NEOLAP_ADMIN_USER and NEOLAP_ADMIN_PASSWORD for the first start, or stop the server and run `neolap user add <file> <name> --role admin`"
            ),
        }
    }
    Ok(AppState::new(
        db,
        config.data_dir.clone(),
        config.insecure_cookies,
        config.trust_proxy,
    ))
}

/// Runs the server until Ctrl+C or SIGTERM (`docker stop`) is received.
pub async fn serve(config: ServerConfig) -> std::io::Result<()> {
    let state = open_state(&config).map_err(|e| std::io::Error::other(e.to_string()))?;
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!(
        "NeOlap {} listening on http://{} (data directory: {}, database: {})",
        neolap_core::VERSION,
        listener.local_addr()?,
        config.data_dir.display(),
        config.database
    );
    if !config.listen.ip().is_loopback() && config.insecure_cookies {
        tracing::warn!(
            "listening on a non-local address with plain-HTTP cookies; use TLS (Caddy profile) outside trusted networks"
        );
    }
    let app = router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
    let closing = state.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            // SSE streams never end by themselves; without this the server would wait for them
            // until `docker stop` kills it, and a running import would be cut off
            closing.begin_shutdown();
        })
        .await?;
    // wait for running jobs: an import is one transaction, so it either completes or never happened
    let mut waited = 0;
    while state.running_jobs() > 0 && waited < 300 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        waited += 1;
    }
    tracing::info!("NeOlap stopped");
    Ok(())
}

/// Completes on Ctrl+C or, on Unix, on SIGTERM – the signal Docker sends on `docker stop`.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                sigterm.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown requested, finishing open requests");
}
