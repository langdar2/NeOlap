//! REST API (`/api/v1/...`), see `docs/Implementierungsplan.md`, chapter 6.3.
//!
//! Every handler checks the role first and the cube rights second, before it reads or changes
//! anything. Blocking engine work runs on the blocking pool so the async runtime stays free.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use neolap_core::cube::{CellValue, Input, WriteContext};
use neolap_core::persist::{Access, CubeRights, Grant, JournalQuery, Role};
use neolap_core::{Cube, MemberId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, Event};

type St = State<Arc<AppState>>;

/// Needs at least `role`.
fn require_role(auth: &Auth, role: Role) -> ApiResult<()> {
    if auth.user.role >= role {
        Ok(())
    } else {
        Err(ApiError::forbidden(format!(
            "this needs the role {} or higher",
            role.as_str()
        )))
    }
}

/// Runs blocking engine work off the async runtime.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
}

/// Rights of the caller on a cube; a cube the caller may not read is reported as not found, so
/// its existence is not revealed.
fn cube_rights(db: &neolap_core::Database, auth: &Auth, cube: &str) -> ApiResult<Arc<CubeRights>> {
    let rights = db
        .rights(&auth.user.name, auth.user.role, cube)
        .map_err(|_| ApiError::not_found(format!("cube '{cube}' not found")))?;
    if rights.can_read_cube() {
        Ok(rights)
    } else {
        Err(ApiError::not_found(format!("cube '{cube}' not found")))
    }
}

fn members_json(dim: &neolap_core::Dimension, ids: &[MemberId]) -> Value {
    Value::Array(
        ids.iter()
            .map(|&m| {
                json!({
                    "id": m,
                    "name": dim.member_name(m),
                    "aliases": dim.aliases(m),
                    "level": dim.level(m),
                    "leaf": dim.is_leaf(m),
                    "children": dim.children(m).len(),
                })
            })
            .collect(),
    )
}

// ---------------------------------------------------------------- model

pub async fn model(State(s): St, auth: Auth) -> ApiResult<Json<Value>> {
    let db = s.db.read();
    let readable = db
        .readable_cubes(&auth.user.name, auth.user.role)
        .map_err(ApiError::from)?;
    let mut dims_used = std::collections::BTreeSet::new();
    let cubes: Vec<Value> = readable
        .iter()
        .filter_map(|n| db.cube(n))
        .map(|c| {
            for d in c.dimensions() {
                dims_used.insert(d.name().to_string());
            }
            json!({
                "name": c.name(),
                "dimensions": c.dimensions().iter().map(|d| d.name()).collect::<Vec<_>>(),
                "generation": c.generation(),
                "cells": c.n_cells(),
            })
        })
        .collect();
    let dimensions: Vec<Value> = dims_used
        .iter()
        .filter_map(|n| db.dimension(n))
        .map(|d| json!({ "name": d.name(), "kind": d.kind(), "members": d.len(), "base_members": d.n_leaves() }))
        .collect();
    Ok(Json(json!({ "cubes": cubes, "dimensions": dimensions })))
}

#[derive(Deserialize)]
pub struct MembersQuery {
    parent: Option<String>,
    search: Option<String>,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

const MAX_PAGE: usize = 1000;

pub async fn members(
    State(s): St,
    auth: Auth,
    Path(dim): Path<String>,
    Query(q): Query<MembersQuery>,
) -> ApiResult<Json<Value>> {
    let db = s.db.read();
    let d = db
        .dimension(&dim)
        .ok_or_else(|| ApiError::not_found(format!("dimension '{dim}' not found")))?;
    // a dimension is visible if the caller may read at least one cube that uses it
    let visible = db
        .readable_cubes(&auth.user.name, auth.user.role)
        .map_err(ApiError::from)?
        .iter()
        .filter_map(|n| db.cube(n))
        .any(|c| c.dimensions().iter().any(|x| x.name() == d.name()));
    if !visible {
        return Err(ApiError::not_found(format!("dimension '{dim}' not found")));
    }
    let limit = q.limit.unwrap_or(100).min(MAX_PAGE);
    let ids: Vec<MemberId> = if let Some(text) = q.search.as_deref().filter(|t| !t.is_empty()) {
        d.search(text, q.offset + limit)
            .into_iter()
            .skip(q.offset)
            .collect()
    } else {
        let parent = match q.parent.as_deref() {
            Some(p) => Some(d.member(p)?),
            None => None,
        };
        d.members_page(parent, q.offset, limit)
    };
    Ok(Json(
        json!({ "members": members_json(d, &ids), "offset": q.offset, "limit": limit }),
    ))
}

#[derive(Deserialize)]
pub struct DimensionPatch {
    ops: Vec<Value>,
}

/// Dimension changes (modeler): `add`, `rename`, `remove`, `move`, `weight`, `alias`, `attr`.
pub async fn patch_dimension(
    State(s): St,
    auth: Auth,
    Path(dim): Path<String>,
    Json(body): Json<DimensionPatch>,
) -> ApiResult<Json<Value>> {
    require_role(&auth, Role::Modeler)?;
    let user = auth.user.name.clone();
    let state = s.clone();
    let n = blocking(move || {
        let mut db = state.db.write();
        let old = db
            .dimension(&dim)
            .cloned()
            .ok_or_else(|| ApiError::not_found(format!("dimension '{dim}' not found")))?;
        let mut ed = old.edit();
        let text = |op: &Value, k: &str| -> ApiResult<String> {
            op.get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| ApiError::bad_request(format!("operation needs '{k}'")))
        };
        let id = |ed: &neolap_core::DimensionEditor, name: &str| {
            ed.resolve(name)
                .ok_or_else(|| ApiError::not_found(format!("member '{name}' not found")))
        };
        for op in &body.ops {
            match op.get("op").and_then(Value::as_str).unwrap_or("") {
                "add" => {
                    let m = ed.add(&text(op, "name")?)?;
                    if let Some(p) = op.get("parent").and_then(Value::as_str) {
                        let p = id(&ed, p)?;
                        ed.add_edge(
                            p,
                            m,
                            op.get("weight").and_then(Value::as_f64).unwrap_or(1.0) as f32,
                        )?;
                    }
                }
                "rename" => {
                    let m = id(&ed, &text(op, "name")?)?;
                    ed.rename(m, &text(op, "to")?)?;
                }
                "remove" => {
                    let m = id(&ed, &text(op, "name")?)?;
                    ed.remove(m)?;
                }
                "move" => {
                    let m = id(&ed, &text(op, "name")?)?;
                    let from = op
                        .get("from")
                        .and_then(Value::as_str)
                        .map(|f| id(&ed, f))
                        .transpose()?;
                    let to = op
                        .get("to")
                        .and_then(Value::as_str)
                        .map(|t| id(&ed, t))
                        .transpose()?;
                    ed.move_member(m, from, to)?;
                }
                "weight" => {
                    let (p, c) = (id(&ed, &text(op, "parent")?)?, id(&ed, &text(op, "name")?)?);
                    let w = op
                        .get("weight")
                        .and_then(Value::as_f64)
                        .ok_or_else(|| ApiError::bad_request("operation needs 'weight'"))?;
                    ed.set_weight(p, c, w as f32)?;
                }
                "alias" => {
                    let m = id(&ed, &text(op, "name")?)?;
                    ed.set_alias(m, &text(op, "alias")?)?;
                }
                "attr" => {
                    let m = id(&ed, &text(op, "name")?)?;
                    let value = match op.get("value") {
                        Some(Value::Number(n)) => {
                            neolap_core::AttrValue::Number(n.as_f64().unwrap_or(0.0))
                        }
                        Some(Value::String(t)) => neolap_core::AttrValue::Text(t.clone()),
                        _ => {
                            return Err(ApiError::bad_request(
                                "attribute value must be a number or text",
                            ));
                        }
                    };
                    ed.set_attribute(&text(op, "attribute")?, m, value)?;
                }
                other => {
                    return Err(ApiError::bad_request(format!(
                        "unknown operation '{other}'"
                    )));
                }
            }
        }
        let new = ed.commit()?;
        let count = body.ops.len();
        db.save_dimension(new, &user)?;
        let cubes: Vec<(String, u64)> = db
            .cubes()
            .filter(|c| {
                c.dimensions()
                    .iter()
                    .any(|d| d.name().eq_ignore_ascii_case(&dim))
            })
            .map(|c| (c.name().to_string(), c.generation()))
            .collect();
        for (cube, generation) in cubes {
            state.publish(Event {
                kind: "model",
                cube: Some(cube),
                generation,
                user: user.clone(),
                cells_changed: 0,
            });
        }
        Ok(count)
    })
    .await?;
    Ok(Json(json!({ "operations": n })))
}

// ---------------------------------------------------------------- views and writes

#[derive(Deserialize)]
pub struct AxisSpec {
    dimension: String,
    members: Vec<String>,
}

#[derive(Deserialize)]
pub struct ViewRequest {
    rows: AxisSpec,
    cols: AxisSpec,
    /// Members of the other dimensions (dimension → member); missing ones default to the root.
    #[serde(default)]
    filters: HashMap<String, String>,
    #[serde(default)]
    suppress_empty: bool,
    #[serde(default)]
    rules: bool,
}

const MAX_VIEW_CELLS: usize = 2_000_000;

pub async fn view(
    State(s): St,
    auth: Auth,
    Path(cube): Path<String>,
    Json(req): Json<ViewRequest>,
) -> ApiResult<Json<Value>> {
    blocking(move || {
        let db = s.db.read();
        let rights = cube_rights(&db, &auth, &cube)?;
        let c = db.cube(&cube).ok_or_else(|| ApiError::not_found(format!("cube '{cube}' not found")))?;
        let dim_index = |name: &str| {
            c.dimensions().iter().position(|d| d.name().eq_ignore_ascii_case(name)).ok_or_else(|| ApiError::bad_request(format!("dimension '{name}' not in cube '{}'", c.name())))
        };
        let (rd, cd) = (dim_index(&req.rows.dimension)?, dim_index(&req.cols.dimension)?);
        if rd == cd {
            return Err(ApiError::bad_request("rows and columns need different dimensions"));
        }
        if req.rows.members.len().saturating_mul(req.cols.members.len()) > MAX_VIEW_CELLS {
            return Err(ApiError::bad_request(format!("a view has at most {MAX_VIEW_CELLS} cells")));
        }
        let resolve = |d: usize, names: &[String]| -> ApiResult<Vec<MemberId>> { names.iter().map(|n| c.dimensions()[d].member(n).map_err(ApiError::from)).collect() };
        let (rows, cols) = (resolve(rd, &req.rows.members)?, resolve(cd, &req.cols.members)?);
        let mut fixed: Vec<MemberId> = c.dimensions().iter().map(|d| d.roots().first().copied().unwrap_or(0)).collect();
        for (dim, member) in &req.filters {
            let d = dim_index(dim)?;
            fixed[d] = c.dimensions()[d].member(member)?;
        }
        let lookup = Lookup(&db);
        let grid = if req.rules { c.view_with(rd, &rows, cd, &cols, &fixed, Some(&lookup))? } else { c.view(rd, &rows, cd, &cols, &fixed)? };
        let mut writable = vec![false; rows.len() * cols.len()];
        let mut kind = vec!["agg"; rows.len() * cols.len()];
        let mut addr = fixed.clone();
        for (ri, &r) in rows.iter().enumerate() {
            for (ci, &k) in cols.iter().enumerate() {
                addr[rd] = r;
                addr[cd] = k;
                let i = ri * cols.len() + ci;
                let base = c.dimensions().iter().zip(&addr).all(|(d, &m)| d.is_leaf(m));
                kind[i] = if grid.errors.contains_key(&i) { "error" } else if c.cell_has_rule(&addr) { "rule" } else if base { "base" } else { "agg" };
                writable[i] = kind[i] != "error" && cell_writable(c, &rights, &addr);
            }
        }
        let keep_rows: Vec<usize> = if req.suppress_empty { grid.non_empty_rows() } else { (0..rows.len()).collect() };
        let keep_cols: Vec<usize> = if req.suppress_empty { grid.non_empty_cols() } else { (0..cols.len()).collect() };
        let pick = |f: &dyn Fn(usize) -> Value| -> Vec<Vec<Value>> { keep_rows.iter().map(|&r| keep_cols.iter().map(|&k| f(r * cols.len() + k)).collect()).collect() };
        let values = pick(&|i| match (grid.cells[i], grid.errors.get(&i)) {
            (_, Some(e)) => json!(e.to_string()),
            (Some(v), _) => json!(v),
            _ => Value::Null,
        });
        Ok(Json(json!({
            "generation": c.generation(),
            "rows": keep_rows.iter().map(|&r| c.dimensions()[rd].member_name(rows[r])).collect::<Vec<_>>(),
            "cols": keep_cols.iter().map(|&k| c.dimensions()[cd].member_name(cols[k])).collect::<Vec<_>>(),
            "values": values,
            "kind": pick(&|i| json!(kind[i])),
            "writable": pick(&|i| json!(writable[i])),
        })))
    })
    .await
}

struct Lookup<'a>(&'a neolap_core::Database);

impl neolap_core::CubeProvider for Lookup<'_> {
    fn cube(&self, name: &str) -> Option<&Cube> {
        self.0.cube(name)
    }
}

/// Whether the caller may write the cell: the engine must accept it (no rule, no weighted
/// aggregate) and, for an aggregate, every base cell below must be writable for the caller
/// (checked over the base area, bounded).
fn cell_writable(c: &Cube, rights: &CubeRights, addr: &[MemberId]) -> bool {
    if rights.best() != Access::Write || !c.is_writable(addr) {
        return false;
    }
    if rights.is_open() {
        return true;
    }
    match c.area_base_coords(addr, 10_000) {
        Some(cells) => cells.iter().all(|k| rights.write_permission_for(k)),
        None => false, // too large to prove; refused rather than guessed
    }
}

#[derive(Deserialize)]
pub struct WriteRequest {
    /// One member per dimension.
    cell: Vec<String>,
    /// Splash syntax: `500`, `#500`, `+50`, `+5%`, `@Ref:500`, empty = delete.
    value: String,
    /// Generation of the client's view.
    generation: Option<u64>,
}

pub async fn write(
    State(s): St,
    auth: Auth,
    Path(cube): Path<String>,
    Json(req): Json<WriteRequest>,
) -> ApiResult<Json<Value>> {
    require_role(&auth, Role::Planner)?;
    let state = s.clone();
    blocking(move || {
        let (rights, addr, input) = {
            let db = s.db.read();
            let rights = cube_rights(&db, &auth, &cube)?;
            let c = db
                .cube(&cube)
                .ok_or_else(|| ApiError::not_found(format!("cube '{cube}' not found")))?;
            let refs: Vec<&str> = req.cell.iter().map(String::as_str).collect();
            (rights, c.address(&refs)?, Input::parse(&req.value)?)
        };
        let mut ctx = WriteContext::new(&auth.user.name);
        ctx.expected_generation = req.generation;
        ctx.can_write = Some(rights.write_permission());
        let mut db = state.db.write();
        let c = db
            .cube_mut(&cube)
            .ok_or_else(|| ApiError::not_found(format!("cube '{cube}' not found")))?;
        let r = c.write(&ctx, &addr, input)?;
        let name = c.name().to_string();
        if r.cells_changed > 0 {
            state.publish(Event {
                kind: "cells",
                cube: Some(name),
                generation: r.generation,
                user: auth.user.name.clone(),
                cells_changed: r.cells_changed,
            });
        }
        Ok(Json(
            json!({ "generation": r.generation, "cells_changed": r.cells_changed }),
        ))
    })
    .await
}

#[derive(Deserialize)]
pub struct UndoRequest {}

pub async fn undo(
    State(s): St,
    auth: Auth,
    Path(cube): Path<String>,
    Json(_): Json<UndoRequest>,
) -> ApiResult<Json<Value>> {
    require_role(&auth, Role::Planner)?;
    let state = s.clone();
    blocking(move || {
        let rights = {
            let db = s.db.read();
            cube_rights(&db, &auth, &cube)?
        };
        let mut ctx = WriteContext::new(&auth.user.name);
        ctx.can_write = Some(rights.write_permission());
        let mut db = state.db.write();
        let c = db
            .cube_mut(&cube)
            .ok_or_else(|| ApiError::not_found(format!("cube '{cube}' not found")))?;
        let r = c.undo(&ctx)?;
        let name = c.name().to_string();
        state.publish(Event {
            kind: "cells",
            cube: Some(name),
            generation: r.generation,
            user: auth.user.name.clone(),
            cells_changed: r.cells_changed,
        });
        Ok(Json(
            json!({ "generation": r.generation, "cells_changed": r.cells_changed }),
        ))
    })
    .await
}

pub async fn cell(
    State(s): St,
    auth: Auth,
    Path(cube): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    blocking(move || {
        let db = s.db.read();
        cube_rights(&db, &auth, &cube)?;
        let c = db.cube(&cube).ok_or_else(|| ApiError::not_found(format!("cube '{cube}' not found")))?;
        let names: Vec<String> = c.dimensions().iter().map(|d| q.get(d.name()).cloned().unwrap_or_default()).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let addr = c.address(&refs)?;
        let lookup = Lookup(&db);
        let v = c.cell(&addr, Some(&lookup))?;
        let value = match v {
            CellValue::Num(n) => json!(n),
            CellValue::Text(t) => json!(t),
            CellValue::Error(e) => json!(e.to_string()),
            CellValue::Empty => Value::Null,
        };
        Ok(Json(json!({ "value": value, "generation": c.generation(), "version": c.cell_version(&addr).ok().flatten().map(|v| json!({ "user": v.user, "generation": v.generation })) })))
    })
    .await
}

// ---------------------------------------------------------------- rules

pub async fn get_rules(
    State(s): St,
    auth: Auth,
    Path(cube): Path<String>,
) -> ApiResult<Json<Value>> {
    let db = s.db.read();
    cube_rights(&db, &auth, &cube)?;
    let c = db
        .cube(&cube)
        .ok_or_else(|| ApiError::not_found(format!("cube '{cube}' not found")))?;
    let status: Vec<Value> = c
        .rule_status()
        .iter()
        .map(|r| json!({ "line": r.line, "valid": r.valid, "message": r.message }))
        .collect();
    Ok(Json(json!({ "text": c.rules_text(), "status": status })))
}

#[derive(Deserialize)]
pub struct RulesBody {
    text: String,
    /// Check only; do not save.
    #[serde(default)]
    check: bool,
}

pub async fn put_rules(
    State(s): St,
    auth: Auth,
    Path(cube): Path<String>,
    Json(body): Json<RulesBody>,
) -> ApiResult<Json<Value>> {
    require_role(&auth, Role::Modeler)?;
    let state = s.clone();
    blocking(move || {
        let mut db = state.db.write();
        let key = db
            .cube(&cube)
            .map(|c| c.name().to_string())
            .ok_or_else(|| ApiError::not_found(format!("cube '{cube}' not found")))?;
        let report = if body.check {
            neolap_core::rules::RuleSet::build(
                &body.text,
                db.cube(&key).map(Cube::dimensions).unwrap_or_default(),
            )?
            .report()
        } else {
            let r = db.set_rules(&key, &body.text, &auth.user.name)?;
            let generation = db.cube(&key).map_or(0, Cube::generation);
            state.publish(Event {
                kind: "rules",
                cube: Some(key.clone()),
                generation,
                user: auth.user.name.clone(),
                cells_changed: 0,
            });
            r
        };
        let status: Vec<Value> = report
            .iter()
            .map(|r| json!({ "line": r.line, "valid": r.valid, "message": r.message }))
            .collect();
        Ok(Json(json!({ "saved": !body.check, "status": status })))
    })
    .await
}

// ---------------------------------------------------------------- journal, export

#[derive(Deserialize)]
pub struct JournalParams {
    cube: Option<String>,
    user: Option<String>,
    since: Option<i64>,
    limit: Option<usize>,
}

pub async fn journal(
    State(s): St,
    auth: Auth,
    Query(q): Query<JournalParams>,
) -> ApiResult<Json<Value>> {
    let db = s.db.read();
    let readable = db
        .readable_cubes(&auth.user.name, auth.user.role)
        .map_err(ApiError::from)?;
    if let Some(c) = &q.cube
        && !readable.iter().any(|r| r.eq_ignore_ascii_case(c))
    {
        return Err(ApiError::not_found(format!("cube '{c}' not found")));
    }
    let entries = db
        .journal(&JournalQuery {
            cube: q.cube.clone(),
            user: q.user.clone(),
            since: q.since,
            limit: Some(q.limit.unwrap_or(100).min(1000)),
        })
        .map_err(ApiError::from)?;
    // Model-wide entries (no cube) are for modelers and admins; cube entries only for readers of that cube.
    let visible: Vec<_> = entries
        .into_iter()
        .filter(|e| {
            if e.cube.is_empty() {
                auth.user.role >= Role::Modeler
            } else {
                readable.iter().any(|r| r.eq_ignore_ascii_case(&e.cube))
            }
        })
        .collect();
    Ok(Json(json!({ "entries": visible })))
}

// ---------------------------------------------------------------- admin

pub async fn list_users(State(s): St, auth: Auth) -> ApiResult<Json<Value>> {
    require_role(&auth, Role::Admin)?;
    let users: Vec<Value> =
        s.db.read()
            .users()?
            .into_iter()
            .map(|u| json!({ "name": u.name, "role": u.role.as_str() }))
            .collect();
    Ok(Json(json!({ "users": users })))
}

#[derive(Deserialize)]
pub struct NewUser {
    name: String,
    password: String,
    role: String,
}

pub async fn add_user(State(s): St, auth: Auth, Json(b): Json<NewUser>) -> ApiResult<Json<Value>> {
    require_role(&auth, Role::Admin)?;
    let role = Role::parse(&b.role)?;
    let state = s.clone();
    blocking(move || {
        state
            .db
            .write()
            .add_user(&b.name, &b.password, role, &auth.user.name)?;
        Ok(Json(json!({ "name": b.name, "role": role.as_str() })))
    })
    .await
}

#[derive(Deserialize)]
pub struct UserPatch {
    role: Option<String>,
    password: Option<String>,
}

pub async fn patch_user(
    State(s): St,
    auth: Auth,
    Path(name): Path<String>,
    Json(b): Json<UserPatch>,
) -> ApiResult<Json<Value>> {
    require_role(&auth, Role::Admin)?;
    let state = s.clone();
    blocking(move || {
        let mut db = state.db.write();
        if let Some(r) = &b.role {
            let role = Role::parse(r)?;
            db.set_role(&name, role, &auth.user.name)?;
            state.refresh_role(&name, role);
        }
        if let Some(p) = &b.password {
            db.set_password(&name, p, &auth.user.name)?;
            state.close_user_sessions(&name);
        }
        Ok(Json(json!({ "name": name })))
    })
    .await
}

pub async fn delete_user(
    State(s): St,
    auth: Auth,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    require_role(&auth, Role::Admin)?;
    let state = s.clone();
    blocking(move || {
        state.db.write().remove_user(&name, &auth.user.name)?;
        state.close_user_sessions(&name);
        Ok(Json(json!({ "removed": name })))
    })
    .await
}

pub async fn create_token(
    State(s): St,
    auth: Auth,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    // a user may create tokens for themselves; creating them for others is an admin task
    if !auth.user.name.eq_ignore_ascii_case(&name) {
        require_role(&auth, Role::Admin)?;
    }
    let state = s.clone();
    blocking(move || {
        let t = state
            .db
            .write()
            .create_token(&name, "api", &auth.user.name)?;
        Ok(Json(json!({ "token": t, "note": "shown only once" })))
    })
    .await
}

pub async fn list_grants(State(s): St, auth: Auth) -> ApiResult<Json<Value>> {
    require_role(&auth, Role::Admin)?;
    Ok(Json(json!({ "grants": s.db.read().grants(None)? })))
}

pub async fn put_grant(State(s): St, auth: Auth, Json(g): Json<Grant>) -> ApiResult<Json<Value>> {
    require_role(&auth, Role::Admin)?;
    s.db.write().grant(&g, &auth.user.name)?;
    Ok(Json(json!({ "granted": true })))
}

#[derive(Deserialize)]
pub struct RevokeBody {
    user: String,
    cube: String,
    #[serde(default)]
    area: Vec<(String, Vec<String>)>,
}

pub async fn delete_grant(
    State(s): St,
    auth: Auth,
    Json(b): Json<RevokeBody>,
) -> ApiResult<Json<Value>> {
    require_role(&auth, Role::Admin)?;
    let n =
        s.db.write()
            .revoke(&b.user, &b.cube, &b.area, &auth.user.name)?;
    Ok(Json(json!({ "revoked": n })))
}

pub async fn backup(State(s): St, auth: Auth) -> ApiResult<Json<Value>> {
    require_role(&auth, Role::Admin)?;
    let state = s.clone();
    blocking(move || {
        let dir = state.data_dir.join("backup");
        std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
        let stamp = neolap_core::persist::timestamp_label();
        let target = dir.join(format!("backup-{stamp}.neolap"));
        state.db.read().backup(&target)?;
        Ok(Json(
            json!({ "file": target.file_name().map(|n| n.to_string_lossy().into_owned()) }),
        ))
    })
    .await
}
