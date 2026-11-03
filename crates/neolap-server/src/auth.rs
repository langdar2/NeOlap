//! Authentication: login with session cookie, API tokens, CSRF protection.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::Extensions;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, extract::Request, middleware::Next};
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::{AppState, Principal};

pub const SESSION_COOKIE: &str = "neolap_session";
pub const CSRF_HEADER: &str = "x-csrf-token";

/// How the request was authenticated; CSRF only matters for cookies.
#[derive(Debug, Clone)]
pub struct Auth {
    pub user: Principal,
    /// CSRF token of the session (cookie authentication), `None` for bearer tokens.
    pub csrf: Option<String>,
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|p| p.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

fn is_unsafe(m: &Method) -> bool {
    !matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Resolves the caller from a bearer token or the session cookie, or fails with 401.
fn authenticate(state: &AppState, headers: &HeaderMap, method: &Method) -> Result<Auth, ApiError> {
    if let Some(token) = bearer(headers) {
        let u = state
            .db
            .read()
            .authenticate_token(token)
            .map_err(|_| ApiError::unauthorized())?;
        return Ok(Auth {
            user: Principal {
                name: u.name,
                role: u.role,
            },
            csrf: None,
        });
    }
    let sid = cookie_value(headers, SESSION_COOKIE).ok_or_else(ApiError::unauthorized)?;
    let (user, csrf) = state.session_user(sid).ok_or_else(ApiError::unauthorized)?;
    if is_unsafe(method) {
        let sent = headers
            .get(CSRF_HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !constant_time_eq(sent.as_bytes(), csrf.as_bytes()) {
            return Err(ApiError::forbidden(
                "missing or wrong CSRF token (header X-CSRF-Token)",
            ));
        }
    }
    Ok(Auth {
        user,
        csrf: Some(csrf),
    })
}

/// Comparison that does not stop at the first differing byte.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl FromRequestParts<Arc<AppState>> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, ApiError> {
        authenticate(state, &parts.headers, &parts.method)
    }
}

/// Sets security headers on every response.
pub async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    h.entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; frame-ancestors 'none'"),
    );
    res
}

#[derive(Deserialize)]
pub struct Login {
    pub user: String,
    pub password: String,
}

/// Peer address of the connection; absent when the router is driven without a socket (tests).
fn peer(ext: &Extensions) -> Option<SocketAddr> {
    ext.get::<ConnectInfo<SocketAddr>>().map(|c| c.0)
}

fn client_key(addr: Option<SocketAddr>, headers: &HeaderMap, trust_proxy: bool) -> String {
    // Behind the Caddy profile every request comes from the proxy; then the forwarded address is
    // the only way to tell clients apart. It is trusted only when the operator says so, and only
    // its *last* entry: a client can send any `X-Forwarded-For` it likes, but our own proxy appends
    // the address it actually saw at the end. The first entry would let an attacker pick a new
    // "client" for every attempt and so defeat the per-client limit.
    if trust_proxy
        && let Some(f) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
        && let Some(last) = f
            .rsplit(',')
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        return last.to_string();
    }
    addr.map_or_else(|| "unknown".into(), |a| a.ip().to_string())
}

pub async fn login(State(state): State<Arc<AppState>>, req: Request) -> Result<Response, ApiError> {
    let (parts, body) = req.into_parts();
    let client = client_key(peer(&parts.extensions), &parts.headers, state.trust_proxy);
    let bytes = axum::body::to_bytes(body, 16 * 1024)
        .await
        .map_err(|_| ApiError::bad_request("login body too large"))?;
    let body: Login = serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::bad_request(format!("login body: {e}")))?;
    if !state.login_allowed(&body.user, &client) {
        return Err(ApiError::too_many(
            "too many failed logins, try again in a few minutes",
        ));
    }
    // The hash is always computed inside `authenticate`; the rate limit comes first only to bound
    // the work an attacker can force.
    let result = tokio::task::spawn_blocking({
        let state = state.clone();
        let (user, pw) = (body.user.clone(), body.password.clone());
        move || state.db.read().authenticate(&user, &pw)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    match result {
        Ok(u) => {
            state.login_succeeded(&u.name);
            let principal = Principal {
                name: u.name.clone(),
                role: u.role,
            };
            let (sid, csrf) = state.open_session(principal);
            let secure = if state.insecure_cookies {
                ""
            } else {
                "; Secure"
            };
            let cookie = format!(
                "{SESSION_COOKIE}={sid}; HttpOnly; SameSite=Strict; Path=/; Max-Age=28800{secure}"
            );
            let mut res = Json(serde_json::json!({
                "user": u.name,
                "role": u.role.as_str(),
                "csrf": csrf,
            }))
            .into_response();
            res.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_str(&cookie).map_err(|e| ApiError::internal(e.to_string()))?,
            );
            Ok(res)
        }
        Err(_) => {
            state.login_failed(&body.user, &client);
            Err(ApiError::unauthorized_msg("wrong user name or password"))
        }
    }
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    _auth: Auth,
) -> Response {
    if let Some(sid) = cookie_value(&headers, SESSION_COOKIE) {
        state.close_session(sid);
    }
    let cookie = format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    let mut res = StatusCode::NO_CONTENT.into_response();
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        res.headers_mut().insert(header::SET_COOKIE, v);
    }
    res
}

/// Who am I (also the way for the UI to get a fresh CSRF token after a reload).
pub async fn me(auth: Auth) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "user": auth.user.name,
        "role": auth.user.role.as_str(),
        "csrf": auth.csrf,
    }))
}
