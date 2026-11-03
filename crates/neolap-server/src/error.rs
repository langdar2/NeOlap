//! API errors: a status code and a JSON body `{ "error": "...", "code": "..." }`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use neolap_core::Error;
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    /// Extra fields for the client (conflict details).
    pub details: Option<serde_json::Value>,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        ApiError {
            status,
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "authentication required",
        )
    }

    pub fn unauthorized_msg(m: &str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", m)
    }

    pub fn forbidden(m: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", m)
    }

    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", m)
    }

    pub fn bad_request(m: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", m)
    }

    pub fn too_many(m: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "too_many_requests", m)
    }

    pub fn internal(m: impl Into<String>) -> Self {
        // the detail goes to the log, not to the client
        tracing::error!("internal error: {}", m.into());
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal error",
        )
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        match &e {
            Error::NotFound { .. } => Self::not_found(e.to_string()),
            Error::Denied(_) => Self::forbidden(e.to_string()),
            Error::WriteConflict { user, cell, value } => ApiError {
                status: StatusCode::CONFLICT,
                code: "conflict",
                message: e.to_string(),
                details: Some(json!({ "user": user, "cell": cell, "value": value })),
            },
            Error::Conflict(_) => Self::new(StatusCode::CONFLICT, "conflict", e.to_string()),
            Error::Parse(_) | Error::InvalidOperation(_) | Error::Cycle { .. } => {
                Self::bad_request(e.to_string())
            }
            Error::Locked(_) => Self::new(StatusCode::SERVICE_UNAVAILABLE, "locked", e.to_string()),
            Error::Storage(_) => Self::internal(e.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = json!({ "error": self.message, "code": self.code });
        if let (Some(d), Some(o)) = (self.details, body.as_object_mut()) {
            o.insert("details".into(), d);
        }
        (self.status, Json(body)).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
