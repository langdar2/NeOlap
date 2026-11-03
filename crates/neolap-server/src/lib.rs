//! NeOlap server: REST API (`/api/v1/...`) and the embedded web UI.
//!
//! Milestone M0 provides the router skeleton, a health endpoint and static
//! delivery of the web UI that is compiled into the binary.

use std::{net::SocketAddr, path::PathBuf};

use axum::{
    Json, Router,
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use rust_embed::RustEmbed;
use serde::Serialize;
use tower_http::trace::TraceLayer;

/// Web UI files from `<repo>/ui`, embedded at compile time.
#[derive(RustEmbed)]
#[folder = "../../ui/"]
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

/// Builds the HTTP router (API + UI).
pub fn router() -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .fallback(api_not_found);
    Router::new()
        .nest("/api/v1", api)
        .fallback(static_file)
        .layer(TraceLayer::new_for_http())
}

/// Server configuration.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Socket address to listen on.
    pub listen: SocketAddr,
    /// Directory for databases, journal and backups (a Docker volume in container setups).
    pub data_dir: PathBuf,
}

/// Runs the server until Ctrl+C or SIGTERM (`docker stop`) is received.
pub async fn serve(config: ServerConfig) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!(
        "NeOlap {} listening on http://{} (data directory: {})",
        neolap_core::VERSION,
        listener.local_addr()?,
        config.data_dir.display()
    );
    axum::serve(listener, router())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, http::Request};
    use tower::ServiceExt;

    async fn get_path(path: &str) -> (StatusCode, String) {
        let response = router()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn health_returns_version() {
        let (status, body) = get_path("/api/v1/health").await;
        assert_eq!(status, StatusCode::OK);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["version"], neolap_core::VERSION);
    }

    #[tokio::test]
    async fn index_page_is_served() {
        let (status, body) = get_path("/").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("NeOlap"));
    }

    #[tokio::test]
    async fn unknown_api_path_is_json_404() {
        let (status, body) = get_path("/api/v1/does-not-exist").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("unknown API endpoint"));
    }
}
