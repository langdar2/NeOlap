//! Imports as background jobs, exports and live events.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderValue, header};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::{Stream, StreamExt};
use neolap_core::cube::WriteContext;
use neolap_core::exchange::{self, ExportSpec, ImportSpec, Layout, SourceKind};
use neolap_core::persist::Role;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, Event, JobStatus};

/// Largest accepted upload. Larger files are imported with the CLI.
pub const MAX_UPLOAD: usize = 256 * 1024 * 1024;

/// What the multipart request of `POST /imports` carries.
struct ImportRequest {
    spec: String,
    file: Option<(String, Vec<u8>)>,
    dry_run: bool,
    /// Generation of the client's view (conflict detection).
    generation: Option<u64>,
}

async fn read_upload(mut mp: Multipart) -> ApiResult<ImportRequest> {
    let mut req = ImportRequest {
        spec: String::new(),
        file: None,
        dry_run: false,
        generation: None,
    };
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("upload: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "spec" => {
                req.spec = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("spec: {e}")))?
            }
            "dry_run" => {
                req.dry_run = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(e.to_string()))?
                    .trim()
                    == "true"
            }
            "generation" => {
                req.generation = field.text().await.ok().and_then(|t| t.trim().parse().ok())
            }
            "file" => {
                let file_name = field.file_name().unwrap_or("upload.csv").to_string();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("file: {e}")))?;
                if bytes.len() > MAX_UPLOAD {
                    return Err(ApiError::bad_request(format!(
                        "the file is larger than {} MB",
                        MAX_UPLOAD / 1024 / 1024
                    )));
                }
                req.file = Some((file_name, bytes.to_vec()));
            }
            _ => {}
        }
    }
    if req.spec.trim().is_empty() {
        return Err(ApiError::bad_request(
            "the field 'spec' (import specification, TOML) is missing",
        ));
    }
    Ok(req)
}

/// `POST /imports`: starts an import (or its preview) as a background job.
pub async fn start_import(
    State(s): State<Arc<AppState>>,
    auth: Auth,
    mp: Multipart,
) -> ApiResult<Json<Value>> {
    if auth.user.role < Role::Planner {
        return Err(ApiError::forbidden("this needs the role planner or higher"));
    }
    let req = read_upload(mp).await?;
    let spec = ImportSpec::from_toml(&req.spec)?;
    let (file_name, data) = req
        .file
        .ok_or_else(|| ApiError::bad_request("the field 'file' is missing"))?;
    let kind = SourceKind::from_path(std::path::Path::new(&file_name));
    // Creating members changes the model: modelers only.
    let creates = matches!(
        spec.options.unknown_member,
        exchange::UnknownPolicy::CreateUnder(_)
    );
    if creates && auth.user.role < Role::Modeler {
        return Err(ApiError::forbidden(
            "an import that creates members needs the role modeler or higher",
        ));
    }
    {
        let db = s.db.read();
        let rights = db
            .rights(&auth.user.name, auth.user.role, &spec.cube)
            .map_err(|_| ApiError::not_found(format!("cube '{}' not found", spec.cube)))?;
        if !rights.can_read_cube() {
            return Err(ApiError::not_found(format!(
                "cube '{}' not found",
                spec.cube
            )));
        }
    }
    let job = s.new_job("import", &auth.user.name);
    let (state, id) = (s.clone(), job.clone());
    let (dry_run, generation) = (req.dry_run, req.generation);
    tokio::task::spawn_blocking(move || {
        let result = run_import_job(
            &state, &id, &auth, &spec, kind, &data, dry_run, generation, creates,
        );
        match result {
            Ok(report) => state.update_job(&id, |j| {
                j.status = if report.get("failed").is_some_and(|f| !f.is_null()) {
                    JobStatus::Failed
                } else {
                    JobStatus::Done
                };
                j.error = report
                    .get("failed")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                j.progress = 100;
                j.phase = "finished".into();
                j.result = Some(report);
                j.finished = Some(std::time::Instant::now());
            }),
            Err(e) => state.update_job(&id, |j| {
                j.status = JobStatus::Failed;
                j.error = Some(e.message);
                j.progress = 100;
                j.phase = "failed".into();
                j.finished = Some(std::time::Instant::now());
            }),
        }
    });
    Ok(Json(json!({ "job": job })))
}

#[allow(clippy::too_many_arguments)]
fn run_import_job(
    state: &Arc<AppState>,
    id: &str,
    auth: &Auth,
    spec: &ImportSpec,
    kind: SourceKind,
    data: &[u8],
    dry_run: bool,
    generation: Option<u64>,
    creates: bool,
) -> ApiResult<Value> {
    let set = |progress: u8, phase: &str| {
        state.update_job(id, |j| {
            j.progress = progress;
            j.phase = phase.into();
        })
    };
    let rights = state
        .db
        .read()
        .rights(&auth.user.name, auth.user.role, &spec.cube)
        .map_err(|_| ApiError::not_found(format!("cube '{}' not found", spec.cube)))?;
    let mut ctx = WriteContext::new(&auth.user.name);
    ctx.expected_generation = generation;
    ctx.can_write = Some(rights.write_permission());

    // Preview, or an import that creates members (changes the model): one step under the write lock.
    if dry_run || creates {
        set(10, if dry_run { "checking" } else { "importing" });
        let report = {
            let mut db = state.db.write();
            db.run_import(spec, kind, data, &ctx, dry_run)?
        };
        if !dry_run {
            publish_import(state, spec, auth, &report);
        }
        return Ok(serde_json::to_value(report).unwrap_or(Value::Null));
    }

    // Normal import: the expensive reading, resolving and checking runs under the *read* lock, so
    // other users keep reading (and the write lock is free for their edits); only the final write
    // takes the exclusive lock briefly. If someone changed the model in between, the import
    // is refused and has to be run again.
    set(10, "reading");
    let prepared = {
        let db = state.db.read();
        let cube = db
            .cube(&spec.cube)
            .ok_or_else(|| ApiError::not_found(format!("cube '{}' not found", spec.cube)))?;
        exchange::prepare_import(cube, spec, kind, data, &ctx)?
    };
    if prepared.report.failed.is_some() {
        return Ok(serde_json::to_value(&prepared.report).unwrap_or(Value::Null));
    }
    set(70, "writing");
    let report = {
        let mut db = state.db.write();
        let cube = db
            .cube_mut(&spec.cube)
            .ok_or_else(|| ApiError::not_found(format!("cube '{}' not found", spec.cube)))?;
        exchange::apply_prepared(cube, prepared, &ctx)?
    };
    publish_import(state, spec, auth, &report);
    Ok(serde_json::to_value(report).unwrap_or(Value::Null))
}

fn publish_import(
    state: &AppState,
    spec: &ImportSpec,
    auth: &Auth,
    report: &exchange::ImportReport,
) {
    if report.failed.is_none() && report.op_id.is_some() {
        state.publish(Event {
            kind: "import",
            cube: Some(spec.cube.clone()),
            generation: report.generation,
            user: auth.user.name.clone(),
            cells_changed: report.cells_created + report.cells_updated + report.cells_deleted,
        });
    }
}

/// `GET /jobs/{id}`: progress and report. Only the owner and admins see a job.
pub async fn job(
    State(s): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    match s.job(&id) {
        Some(j)
            if j.owner.eq_ignore_ascii_case(&auth.user.name) || auth.user.role == Role::Admin =>
        {
            Ok(Json(serde_json::to_value(j).unwrap_or(Value::Null)))
        }
        _ => Err(ApiError::not_found("job not found")),
    }
}

#[derive(Deserialize)]
pub struct ExportParams {
    area: Option<String>,
    wide: Option<String>,
    #[serde(default)]
    aggregates: bool,
    #[serde(default)]
    rules: bool,
    format: Option<String>,
    decimal: Option<String>,
}

/// `GET /cubes/{c}/export`: CSV or xlsx.
pub async fn export(
    State(s): State<Arc<AppState>>,
    auth: Auth,
    Path(cube): Path<String>,
    Query(q): Query<ExportParams>,
) -> ApiResult<Response> {
    let (bytes, content_type, file) =
        tokio::task::spawn_blocking(move || -> ApiResult<(Vec<u8>, &'static str, String)> {
            let db = s.db.read();
            let rights = db
                .rights(&auth.user.name, auth.user.role, &cube)
                .map_err(|_| ApiError::not_found(format!("cube '{cube}' not found")))?;
            if !rights.can_read_cube() {
                return Err(ApiError::not_found(format!("cube '{cube}' not found")));
            }
            let c = db
                .cube(&cube)
                .ok_or_else(|| ApiError::not_found(format!("cube '{cube}' not found")))?;
            let area = q
                .area
                .as_deref()
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let (d, m) = p.split_once('=').ok_or_else(|| {
                        ApiError::bad_request(format!(
                            "area part '{p}' needs the form dimension=member|member"
                        ))
                    })?;
                    Ok((
                        d.trim().to_string(),
                        m.split('|').map(|x| x.trim().to_string()).collect(),
                    ))
                })
                .collect::<ApiResult<Vec<_>>>()?;
            let spec = ExportSpec {
                area,
                layout: q
                    .wide
                    .clone()
                    .map_or(Layout::Long, |dimension| Layout::Wide { dimension }),
                aggregates: q.aggregates,
                with_rules: q.rules,
                suppress_empty: true,
            };
            struct Lookup<'a>(&'a neolap_core::Database);
            impl neolap_core::CubeProvider for Lookup<'_> {
                fn cube(&self, name: &str) -> Option<&neolap_core::Cube> {
                    self.0.cube(name)
                }
            }
            let table = exchange::export_table(c, &spec, Some(&Lookup(&db)))?;
            let name = c.name().to_string();
            if q.format.as_deref() == Some("xlsx") {
                Ok((
                    exchange::to_xlsx(&table, &name)?,
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                    format!("{name}.xlsx"),
                ))
            } else {
                let dec = if q.decimal.as_deref() == Some(",") {
                    ','
                } else {
                    '.'
                };
                Ok((
                    exchange::to_csv(&table, if dec == ',' { b';' } else { b',' }, dec)?,
                    "text/csv; charset=utf-8",
                    format!("{name}.csv"),
                ))
            }
        })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))??;
    let safe: String = file
        .chars()
        .filter(|c| c.is_alphanumeric() || "._-".contains(*c))
        .collect();
    let mut res = bytes.into_response();
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{safe}\"")) {
        res.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(res)
}

/// `GET /events`: Server-Sent Events for cubes the caller may read. A client that lags too far
/// behind gets a `resync` event and should reload its view.
pub async fn events(
    State(s): State<Arc<AppState>>,
    auth: Auth,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = s.events.subscribe();
    let state = s.clone();
    let closing = s.clone();
    let stream = BroadcastStream::new(rx)
        .take_until(async move { closing.shutdown_requested().await })
        .filter_map(move |item| {
            let state = state.clone();
            let auth = auth.clone();
            async move {
                match item {
                    Ok(ev) => {
                        // events of cubes the caller may not read are not sent (their existence stays hidden)
                        let allowed = match &ev.cube {
                            None => true,
                            Some(c) => state
                                .db
                                .read()
                                .rights(&auth.user.name, auth.user.role, c)
                                .is_ok_and(|r| r.can_read_cube()),
                        };
                        allowed.then(|| {
                            Ok(SseEvent::default()
                                .event(ev.kind)
                                .json_data(&ev)
                                .unwrap_or_default())
                        })
                    }
                    Err(BroadcastStreamRecvError::Lagged(n)) => {
                        Some(Ok(SseEvent::default().event("resync").data(n.to_string())))
                    }
                }
            }
        });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20)))
}
