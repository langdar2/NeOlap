//! Load test: 20 simulated users reading views and writing cells at the same time.
//!
//! `cargo test --release -p neolap-server --test load -- --nocapture`
//! (also runs in debug mode, with fewer rounds, as a concurrency check)

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use neolap_core::persist::{Grant, Role};
use neolap_core::{Database, Dimension, DimensionKind};
use neolap_server::AppState;
use serde_json::{Value, json};
use tower::ServiceExt;

const USERS: usize = 20;
const PW: &str = "a very long secret";

async fn call(
    app: &Router,
    cookie: &str,
    csrf: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value, Duration) {
    let mut b = Request::builder()
        .method(method)
        .uri(format!("/api/v1{path}"))
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", csrf);
    let req = match body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            b.body(Body::from(v.to_string())).unwrap()
        }
        None => b.body(Body::empty()).unwrap(),
    };
    let start = Instant::now();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        start.elapsed(),
    )
}

async fn login(app: &Router, user: &str) -> (String, String) {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "user": user, "password": PW }).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let cookie = res.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let v: Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (cookie, v["csrf"].as_str().unwrap().to_string())
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() as f64 * p) as usize).min(sorted.len() - 1)]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn twenty_users_read_and_write_without_losing_a_change() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::create(dir.path().join("load.neolap")).unwrap();
    // a model of realistic shape: 1,000 products × 12 months × 20 regions (one per user)
    let mut e = Dimension::new("products", DimensionKind::Generic).edit();
    let all = e.add("All").unwrap();
    for g in 0..20 {
        let grp = e.add(&format!("g{g}")).unwrap();
        e.add_edge(all, grp, 1.0).unwrap();
        for p in 0..50 {
            let m = e.add(&format!("p{g}_{p}")).unwrap();
            e.add_edge(grp, m, 1.0).unwrap();
        }
    }
    db.save_dimension(e.commit().unwrap(), "s").unwrap();
    let mut e = Dimension::new("regions", DimensionKind::Generic).edit();
    let all = e.add("Total").unwrap();
    for u in 0..USERS {
        let r = e.add(&format!("r{u}")).unwrap();
        e.add_edge(all, r, 1.0).unwrap();
    }
    db.save_dimension(e.commit().unwrap(), "s").unwrap();
    let mut e = Dimension::new("months", DimensionKind::Time).edit();
    let all = e.add("Year").unwrap();
    for m in 1..=12 {
        let m = e.add(&format!("m{m}")).unwrap();
        e.add_edge(all, m, 1.0).unwrap();
    }
    db.save_dimension(e.commit().unwrap(), "s").unwrap();
    db.create_cube("plan", &["products", "regions", "months"], "s")
        .unwrap();
    // 200,000 filled cells so views and splashes have real work to do
    {
        let cube = db.cube_mut("plan").unwrap();
        let ctx = neolap_core::WriteContext::new("seed");
        let mut cells = Vec::new();
        for u in 0..USERS {
            for g in 0..20 {
                for p in 0..50 {
                    for m in [1, 6] {
                        let a = cube
                            .address(&[&format!("p{g}_{p}"), &format!("r{u}"), &format!("m{m}")])
                            .unwrap();
                        cells.push((a, 1.0 + (u + g + p) as f64));
                    }
                }
            }
        }
        cube.bulk_write(
            &ctx,
            neolap_core::cube::BulkWrite {
                clear: None,
                cells,
                mode: neolap_core::cube::BulkMode::Overwrite,
                dry_run: false,
            },
        )
        .unwrap();
    }
    for u in 0..USERS {
        db.add_user(&format!("u{u}"), PW, Role::Planner, "s")
            .unwrap();
        let g: Grant = serde_json::from_value(json!({ "user": format!("u{u}"), "cube": "plan", "access": "write", "area": [["regions", [format!("r{u}")]]] })).unwrap();
        db.grant(&g, "s").unwrap();
    }
    let state = AppState::new(db, dir.path().to_path_buf(), true, false);
    let app = neolap_server::router(state.clone());
    let mut sessions = Vec::new();
    for u in 0..USERS {
        sessions.push(login(&app, &format!("u{u}")).await);
    }

    let rounds = if cfg!(debug_assertions) { 10 } else { 60 };
    let started = Instant::now();
    let mut tasks = Vec::new();
    for (u, (cookie, csrf)) in sessions.into_iter().enumerate() {
        let app = app.clone();
        tasks.push(tokio::spawn(async move {
            let (mut reads, mut writes) = (Vec::new(), Vec::new());
            let mut expected = std::collections::HashMap::new();
            for round in 0..rounds {
                // 4 views of a pivot (50 products × 12 months) for every write, like a user clicking around
                for _ in 0..4 {
                    let req = json!({
                        "rows": { "dimension": "products", "members": (0..50).map(|p| format!("p{}_{p}", u % 20)).collect::<Vec<_>>() },
                        "cols": { "dimension": "months", "members": ["m1", "m6", "Year"] },
                        "filters": { "regions": "Total" },
                        "suppress_empty": true,
                    });
                    let (s, v, t) = call(&app, &cookie, &csrf, "POST", "/cubes/plan/view", Some(req)).await;
                    assert_eq!(s, StatusCode::OK, "user {u}: {v}");
                    reads.push(t);
                }
                let (g, p, m) = (round % 20, round % 50, if round % 2 == 0 { "m1" } else { "m6" });
                let value = 1000 + (u * 1000 + round) as i64;
                let body = json!({ "cell": [format!("p{g}_{p}"), format!("r{u}"), m], "value": value.to_string() });
                let (s, v, t) = call(&app, &cookie, &csrf, "POST", "/cubes/plan/write", Some(body)).await;
                assert_eq!(s, StatusCode::OK, "user {u} round {round}: {v}");
                writes.push(t);
                expected.insert((g, p, m), value);
            }
            (reads, writes, expected)
        }));
    }
    let (mut reads, mut writes) = (Vec::new(), Vec::new());
    let mut all_expected = Vec::new();
    for (u, t) in tasks.into_iter().enumerate() {
        let (r, w, e) = t.await.unwrap();
        reads.extend(r);
        writes.extend(w);
        all_expected.push((u, e));
    }
    let total = started.elapsed();
    reads.sort();
    writes.sort();
    println!(
        "{USERS} users, {rounds} rounds each: {} views + {} writes in {total:?}\n  views : median {:?}, p95 {:?}, max {:?}\n  writes: median {:?}, p95 {:?}, max {:?}",
        reads.len(),
        writes.len(),
        percentile(&reads, 0.5),
        percentile(&reads, 0.95),
        reads.last().unwrap(),
        percentile(&writes, 0.5),
        percentile(&writes, 0.95),
        writes.last().unwrap(),
    );

    // nothing was lost: every user's last value of each cell is what the cube holds, and every
    // write was journaled
    let db = state.db.read();
    let cube = db.cube("plan").unwrap();
    for (u, expected) in &all_expected {
        for ((g, p, m), value) in expected {
            let a = cube
                .address(&[&format!("p{g}_{p}"), &format!("r{u}"), m])
                .unwrap();
            assert_eq!(
                cube.value(&a).unwrap(),
                Some(*value as f64),
                "user {u} lost a write"
            );
        }
    }
    let journal = db
        .journal(&neolap_core::persist::JournalQuery {
            cube: Some("plan".into()),
            limit: Some(1_000_000),
            ..Default::default()
        })
        .unwrap();
    let splashes = journal.iter().filter(|e| e.op == "splash").count();
    assert_eq!(
        splashes,
        USERS * rounds,
        "every write is in the journal exactly once"
    );
    let generations: std::collections::HashSet<_> = journal
        .iter()
        .filter(|e| e.op == "splash")
        .map(|e| e.generation)
        .collect();
    assert_eq!(
        generations.len(),
        splashes,
        "every write got its own generation"
    );
    drop(db);

    if !cfg!(debug_assertions) {
        assert!(
            percentile(&reads, 0.95) < Duration::from_millis(500),
            "view p95 over the plan's 500 ms"
        );
        assert!(
            percentile(&writes, 0.95) < Duration::from_secs(2),
            "write p95 over the plan's 2 s"
        );
    }
    let _: Arc<AppState> = state;
}

/// All users write the *same* cell, each with the generation of the view they saw. Of every group
/// that saw the same generation exactly one write may win; the others get a conflict and must lose
/// nothing but their own attempt. Without this check a multi-user test proves little, since in
/// the test above nobody ever touches the same cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn simultaneous_writes_to_one_cell_produce_exactly_one_winner_per_generation() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::create(dir.path().join("c.neolap")).unwrap();
    let mut e = Dimension::new("d", DimensionKind::Generic).edit();
    let all = e.add("All").unwrap();
    e.add_children(all, &["x"]).unwrap();
    db.save_dimension(e.commit().unwrap(), "s").unwrap();
    db.create_cube("c", &["d"], "s").unwrap();
    for u in 0..USERS {
        db.add_user(&format!("u{u}"), PW, Role::Modeler, "s")
            .unwrap();
    }
    let state = AppState::new(db, dir.path().to_path_buf(), true, false);
    let app = neolap_server::router(state.clone());
    let mut sessions = Vec::new();
    for u in 0..USERS {
        sessions.push(login(&app, &format!("u{u}")).await);
    }
    let rounds = 25;
    let mut total_ok = 0;
    let mut total_conflict = 0;
    for round in 0..rounds {
        let (cookie, csrf) = &sessions[0];
        let (_, v, _) = call(&app, cookie, csrf, "GET", "/cubes/c/cell?d=x", None).await;
        let seen = v["generation"].as_u64().unwrap();
        let barrier = Arc::new(tokio::sync::Barrier::new(USERS));
        let mut tasks = Vec::new();
        for (u, (cookie, csrf)) in sessions.iter().cloned().enumerate() {
            let (app, barrier) = (app.clone(), barrier.clone());
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                let body = json!({ "cell": ["x"], "value": format!("{}", round * 100 + u + 1), "generation": seen });
                let (s, v, _) = call(&app, &cookie, &csrf, "POST", "/cubes/c/write", Some(body)).await;
                (s, v)
            }));
        }
        let (mut ok, mut conflict) = (0, 0);
        for t in tasks {
            let (s, v) = t.await.unwrap();
            match s {
                StatusCode::OK => ok += 1,
                StatusCode::CONFLICT => {
                    assert_eq!(v["code"], "conflict");
                    assert!(
                        v["details"]["user"].as_str().unwrap().starts_with('u'),
                        "the conflict names who won: {v}"
                    );
                    conflict += 1;
                }
                other => panic!("unexpected {other}: {v}"),
            }
        }
        assert_eq!((ok, conflict), (1, USERS - 1), "round {round}");
        total_ok += ok;
        total_conflict += conflict;
    }
    println!(
        "{rounds} rounds × {USERS} writers on one cell: {total_ok} winners, {total_conflict} conflicts"
    );
    let db = state.db.read();
    let journal = db
        .journal(&neolap_core::persist::JournalQuery {
            cube: Some("c".into()),
            limit: Some(10_000),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        journal.iter().filter(|e| e.op == "splash").count(),
        rounds,
        "only the winners are journaled"
    );
}
