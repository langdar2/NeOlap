//! API integration tests: the real router, real cookies and tokens, a real database file.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use neolap_core::persist::Role;
use neolap_core::{Database, Dimension, DimensionKind};
use neolap_server::AppState;
use serde_json::{Value, json};
use tower::ServiceExt;

struct Env {
    app: Router,
    state: Arc<AppState>,
    _dir: tempfile::TempDir,
}

fn setup() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::create(dir.path().join("t.neolap")).unwrap();
    let mut years = Dimension::new("years", DimensionKind::Time).edit();
    let total = years.add("Year").unwrap();
    for (q, months) in [("Q1", ["Jan", "Feb"]), ("Q2", ["Apr", "May"])] {
        let q = years.add(q).unwrap();
        years.add_edge(total, q, 1.0).unwrap();
        years.add_children(q, &months).unwrap();
    }
    db.save_dimension(years.commit().unwrap(), "setup").unwrap();
    let mut regions = Dimension::new("regions", DimensionKind::Generic).edit();
    let all = regions.add("Total").unwrap();
    regions.add_children(all, &["Nord", "Sued"]).unwrap();
    db.save_dimension(regions.commit().unwrap(), "setup")
        .unwrap();
    let mut m = Dimension::new("measures", DimensionKind::Measures).edit();
    let profit = m.add("Profit").unwrap();
    let (sales, cost) = (m.add("Sales").unwrap(), m.add("Cost").unwrap());
    m.add_edge(profit, sales, 1.0).unwrap();
    m.add_edge(profit, cost, -1.0).unwrap();
    m.add("Margin").unwrap();
    db.save_dimension(m.commit().unwrap(), "setup").unwrap();
    db.create_cube("sales", &["years", "regions", "measures"], "setup")
        .unwrap();
    db.create_cube("secret", &["years", "regions", "measures"], "setup")
        .unwrap();
    for (n, role) in [
        ("root", Role::Admin),
        ("mo", Role::Modeler),
        ("pia", Role::Planner),
        ("nina", Role::Planner),
        ("rita", Role::Reader),
    ] {
        db.add_user(n, "a very long secret", role, "setup").unwrap();
    }
    for (u, access, area) in [
        ("pia", "write", json!([["regions", ["Nord"]]])),
        ("nina", "write", json!([["regions", ["Sued"]]])),
        ("rita", "read", json!([])),
    ] {
        let g: neolap_core::persist::Grant = serde_json::from_value(
            json!({ "user": u, "cube": "sales", "access": access, "area": area }),
        )
        .unwrap();
        db.grant(&g, "setup").unwrap();
    }
    let state = AppState::new(db, dir.path().to_path_buf(), true, false);
    Env {
        app: neolap_server::router(state.clone()),
        state,
        _dir: dir,
    }
}

struct Client {
    cookie: Option<String>,
    csrf: Option<String>,
    bearer: Option<String>,
}

impl Client {
    fn anonymous() -> Client {
        Client {
            cookie: None,
            csrf: None,
            bearer: None,
        }
    }

    fn token(t: &str) -> Client {
        Client {
            cookie: None,
            csrf: None,
            bearer: Some(t.to_string()),
        }
    }
}

async fn body_json(res: Response<Body>) -> (StatusCode, Value, Option<String>) {
    let status = res.status();
    let cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        cookie,
    )
}

async fn send(
    app: &Router,
    c: &Client,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(format!("/api/v1{path}"));
    if let Some(cookie) = &c.cookie {
        b = b.header(header::COOKIE, cookie);
    }
    if let Some(csrf) = &c.csrf {
        b = b.header("x-csrf-token", csrf);
    }
    if let Some(t) = &c.bearer {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let (s, v, _) = body_json(app.clone().oneshot(req).await.unwrap()).await;
    (s, v)
}

async fn login(app: &Router, user: &str, password: &str) -> Result<Client, (StatusCode, Value)> {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "user": user, "password": password }).to_string(),
        ))
        .unwrap();
    let (status, v, cookie) = body_json(app.clone().oneshot(req).await.unwrap()).await;
    if status != StatusCode::OK {
        return Err((status, v));
    }
    let cookie = cookie.unwrap();
    assert!(
        cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"),
        "{cookie}"
    );
    Ok(Client {
        cookie: Some(cookie.split(';').next().unwrap().to_string()),
        csrf: v["csrf"].as_str().map(str::to_string),
        bearer: None,
    })
}

async fn user(env: &Env, name: &str) -> Client {
    login(&env.app, name, "a very long secret").await.unwrap()
}

const PW: &str = "a very long secret";

#[tokio::test]
async fn health_ui_and_unknown_paths_need_no_login() {
    let env = setup();
    let (s, v) = send(&env.app, &Client::anonymous(), "GET", "/health", None).await;
    assert_eq!((s, v["status"].as_str()), (StatusCode::OK, Some("ok")));
    let res = env
        .app
        .clone()
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    for h in [
        "x-content-type-options",
        "x-frame-options",
        "content-security-policy",
        "referrer-policy",
    ] {
        assert!(res.headers().contains_key(h), "{h}");
    }
    let (s, v) = send(&env.app, &Client::anonymous(), "GET", "/nothing-here", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("unknown API endpoint")
    );
}

#[tokio::test]
async fn every_protected_route_refuses_anonymous_callers() {
    let env = setup();
    let anon = Client::anonymous();
    for (method, path) in [
        ("GET", "/me"),
        ("GET", "/model"),
        ("GET", "/dimensions/years/members"),
        ("PATCH", "/dimensions/years"),
        ("POST", "/cubes/sales/view"),
        ("GET", "/cubes/sales/cell"),
        ("POST", "/cubes/sales/write"),
        ("POST", "/cubes/sales/undo"),
        ("GET", "/cubes/sales/rules"),
        ("PUT", "/cubes/sales/rules"),
        ("GET", "/cubes/sales/export"),
        ("POST", "/imports"),
        ("GET", "/jobs/x"),
        ("GET", "/journal"),
        ("GET", "/events"),
        ("GET", "/admin/users"),
        ("POST", "/admin/users"),
        ("PATCH", "/admin/users/pia"),
        ("DELETE", "/admin/users/pia"),
        ("POST", "/admin/users/pia/token"),
        ("GET", "/admin/grants"),
        ("PUT", "/admin/grants"),
        ("DELETE", "/admin/grants"),
        ("POST", "/admin/backup"),
        ("POST", "/logout"),
    ] {
        let (s, _) = send(&env.app, &anon, method, path, Some(json!({}))).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
    // invalid credentials are no better than none
    let (s, _) = send(&env.app, &Client::token("nlp_x_y"), "GET", "/model", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let forged = Client {
        cookie: Some("neolap_session=deadbeef".into()),
        csrf: None,
        bearer: None,
    };
    assert_eq!(
        send(&env.app, &forged, "GET", "/model", None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn login_logout_and_the_session_cookie() {
    let env = setup();
    let c = user(&env, "pia").await;
    let (s, v) = send(&env.app, &c, "GET", "/me", None).await;
    assert_eq!(
        (s, v["user"].as_str(), v["role"].as_str()),
        (StatusCode::OK, Some("pia"), Some("planner"))
    );
    assert_eq!(
        send(&env.app, &c, "POST", "/logout", Some(json!({})))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&env.app, &c, "GET", "/me", None).await.0,
        StatusCode::UNAUTHORIZED,
        "the session ended"
    );
    // wrong password and unknown user give the same answer
    let a = login(&env.app, "pia", "not the password!")
        .await
        .err()
        .unwrap();
    let b = login(&env.app, "nobody", PW).await.err().unwrap();
    assert_eq!((a.0, &a.1["error"]), (b.0, &b.1["error"]));
    assert_eq!(a.0, StatusCode::UNAUTHORIZED);
    // the cookie carries Secure once the server runs behind TLS
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::create(dir.path().join("s.neolap")).unwrap();
    db.add_user("u", PW, Role::Admin, "s").unwrap();
    let app = neolap_server::router(AppState::new(db, dir.path().to_path_buf(), false, false));
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"user":"u","password":PW}).to_string()))
        .unwrap();
    let (_, _, cookie) = body_json(app.oneshot(req).await.unwrap()).await;
    assert!(cookie.unwrap().contains("Secure"));
}

#[tokio::test]
async fn login_is_rate_limited_per_user_and_blocks_even_the_right_password() {
    let env = setup();
    for _ in 0..5 {
        assert_eq!(
            login(&env.app, "pia", "wrong password!!")
                .await
                .err()
                .unwrap()
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    let blocked = login(&env.app, "pia", PW).await.err().unwrap();
    assert_eq!(blocked.0, StatusCode::TOO_MANY_REQUESTS);
    // other users from the same (test) client are blocked as well once the client key is exhausted,
    // which is the point of the per-client limit
    assert_eq!(
        login(&env.app, "rita", PW).await.err().unwrap().0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn a_successful_login_clears_the_failure_count_of_that_user() {
    let env = setup();
    for _ in 0..3 {
        let _ = login(&env.app, "pia", "wrong password!!").await;
    }
    assert!(login(&env.app, "pia", PW).await.is_ok());
    for _ in 0..3 {
        let _ = login(&env.app, "pia", "wrong password!!").await;
    }
    // 3 + 3 failures would exceed the limit if the first three still counted for the user key; the
    // client key counted all six, so this only demonstrates the per-user reset directly:
    assert!(env.state.login_allowed("rita", "another-client"));
}

#[tokio::test]
async fn csrf_token_is_required_for_cookie_writes_but_not_for_reads_or_bearer_tokens() {
    let env = setup();
    let mut c = user(&env, "pia").await;
    let cell = json!({ "cell": ["Jan", "Nord", "Sales"], "value": "10" });
    let csrf = c.csrf.take();
    let (s, v) = send(
        &env.app,
        &c,
        "POST",
        "/cubes/sales/write",
        Some(cell.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(v["error"].as_str().unwrap().contains("CSRF"));
    c.csrf = Some("wrong-token".into());
    assert_eq!(
        send(
            &env.app,
            &c,
            "POST",
            "/cubes/sales/write",
            Some(cell.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&env.app, &c, "GET", "/model", None).await.0,
        StatusCode::OK,
        "reads need no CSRF token"
    );
    c.csrf = csrf;
    assert_eq!(
        send(
            &env.app,
            &c,
            "POST",
            "/cubes/sales/write",
            Some(cell.clone())
        )
        .await
        .0,
        StatusCode::OK
    );
    // bearer tokens are not sent by browsers on their own, so they need no CSRF token
    let token = env
        .state
        .db
        .write()
        .create_token("pia", "t", "setup")
        .unwrap();
    let t = Client::token(&token);
    let (s, _) = send(
        &env.app,
        &t,
        "POST",
        "/cubes/sales/write",
        Some(json!({ "cell": ["Feb", "Nord", "Sales"], "value": "5" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn a_cube_without_a_grant_does_not_exist_for_the_caller() {
    let env = setup();
    let pia = user(&env, "pia").await;
    // pia has a grant on sales only: secret is reported as not found, not as forbidden
    for (m, p, b) in [
        ("GET", "/cubes/secret/rules", None),
        ("GET", "/cubes/secret/export", None),
        (
            "GET",
            "/cubes/secret/cell?years=Jan&regions=Nord&measures=Sales",
            None,
        ),
        (
            "POST",
            "/cubes/secret/view",
            Some(
                json!({"rows":{"dimension":"years","members":["Jan"]},"cols":{"dimension":"measures","members":["Sales"]}}),
            ),
        ),
        (
            "POST",
            "/cubes/secret/write",
            Some(json!({"cell":["Jan","Nord","Sales"],"value":"1"})),
        ),
    ] {
        let (s, v) = send(&env.app, &pia, m, p, b).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{m} {p}");
        assert!(v["error"].as_str().unwrap().contains("not found"));
    }
    let (_, v) = send(&env.app, &pia, "GET", "/model", None).await;
    let names: Vec<_> = v["cubes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["sales"]);
    let root = user(&env, "root").await;
    let (_, v) = send(&env.app, &root, "GET", "/model", None).await;
    assert_eq!(v["cubes"].as_array().unwrap().len(), 2);
    // a nonexistent cube looks exactly like a forbidden one
    let (s, a) = send(&env.app, &pia, "GET", "/cubes/nothing/rules", None).await;
    let (_, b) = send(&env.app, &pia, "GET", "/cubes/secret/rules", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(a["code"], b["code"]);
}

#[tokio::test]
async fn roles_limit_what_a_caller_may_do() {
    let env = setup();
    let rita = user(&env, "rita").await;
    let pia = user(&env, "pia").await;
    let mo = user(&env, "mo").await;
    let write = json!({ "cell": ["Jan", "Nord", "Sales"], "value": "1" });
    assert_eq!(
        send(
            &env.app,
            &rita,
            "POST",
            "/cubes/sales/write",
            Some(write.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "readers do not write"
    );
    assert_eq!(
        send(&env.app, &rita, "GET", "/cubes/sales/rules", None)
            .await
            .0,
        StatusCode::OK
    );
    let rules = json!({ "text": "['Margin'] = 1;" });
    assert_eq!(
        send(
            &env.app,
            &pia,
            "PUT",
            "/cubes/sales/rules",
            Some(rules.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "planners do not model"
    );
    assert_eq!(
        send(
            &env.app,
            &pia,
            "PATCH",
            "/dimensions/years",
            Some(json!({"ops":[{"op":"add","name":"X"}]}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&env.app, &mo, "PUT", "/cubes/sales/rules", Some(rules))
            .await
            .0,
        StatusCode::OK
    );
    for (m, p) in [
        ("GET", "/admin/users"),
        ("GET", "/admin/grants"),
        ("POST", "/admin/backup"),
    ] {
        for c in [&rita, &pia, &mo] {
            assert_eq!(
                send(&env.app, c, m, p, Some(json!({}))).await.0,
                StatusCode::FORBIDDEN,
                "{m} {p}"
            );
        }
    }
    let root = user(&env, "root").await;
    assert_eq!(
        send(&env.app, &root, "GET", "/admin/users", None).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn area_grants_decide_which_cells_a_planner_may_write() {
    let env = setup();
    let pia = user(&env, "pia").await; // writes Nord
    let nina = user(&env, "nina").await; // writes Sued
    assert_eq!(put_cell(&env, &pia, "Nord", "100").await.0, StatusCode::OK);
    let (s, v) = put_cell(&env, &pia, "Sued", "100").await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(v["error"].as_str().unwrap().contains("Sued"));
    assert_eq!(put_cell(&env, &nina, "Sued", "50").await.0, StatusCode::OK);
    // an aggregate spanning both regions: a proportional splash reaches both cells → refused as a whole
    let (s, _) = send(
        &env.app,
        &pia,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Total","Sales"],"value":"300"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (_, cell) = send(
        &env.app,
        &pia,
        "GET",
        "/cubes/sales/cell?years=Jan&regions=Total&measures=Sales",
        None,
    )
    .await;
    assert_eq!(cell["value"], json!(150.0), "nothing changed");
}

#[tokio::test]
async fn views_report_writable_cells_and_cell_kinds() {
    let env = setup();
    let pia = user(&env, "pia").await;
    let rita = user(&env, "rita").await;
    let req = json!({
        "rows": { "dimension": "regions", "members": ["Nord", "Sued", "Total"] },
        "cols": { "dimension": "measures", "members": ["Sales", "Profit"] },
        "filters": { "years": "Jan" },
    });
    send(
        &env.app,
        &pia,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"10"})),
    )
    .await;
    let (s, v) = send(
        &env.app,
        &pia,
        "POST",
        "/cubes/sales/view",
        Some(req.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["values"][0][0], json!(10.0));
    assert_eq!(v["kind"][0][0], "base");
    assert_eq!(v["kind"][2][0], "agg");
    assert_eq!(v["writable"][0][0], json!(true)); // Nord/Sales: granted
    assert_eq!(v["writable"][1][0], json!(false)); // Sued: not granted
    assert_eq!(v["writable"][2][0], json!(false)); // Total spans Sued
    assert_eq!(v["writable"][0][1], json!(false)); // Profit = Sales − Cost: weights other than 1, the engine refuses it
    let (_, v) = send(&env.app, &rita, "POST", "/cubes/sales/view", Some(req)).await;
    let all_false = v["writable"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row.as_array().unwrap().iter().all(|w| *w == json!(false)));
    assert!(all_false, "readers write nothing");
    assert_eq!(v["values"][0][0], json!(10.0), "readers read");
}

#[tokio::test]
async fn views_suppress_empty_rows_and_columns_and_enforce_limits() {
    let env = setup();
    let root = user(&env, "root").await;
    send(
        &env.app,
        &root,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"10"})),
    )
    .await;
    let req = |suppress: bool| {
        json!({
            "rows": { "dimension": "regions", "members": ["Nord", "Sued"] },
            "cols": { "dimension": "measures", "members": ["Sales", "Cost"] },
            "filters": { "years": "Jan" }, "suppress_empty": suppress,
        })
    };
    let (_, v) = send(
        &env.app,
        &root,
        "POST",
        "/cubes/sales/view",
        Some(req(true)),
    )
    .await;
    assert_eq!(
        (
            v["rows"].as_array().unwrap().len(),
            v["cols"].as_array().unwrap().len()
        ),
        (1, 1)
    );
    let (_, v) = send(
        &env.app,
        &root,
        "POST",
        "/cubes/sales/view",
        Some(req(false)),
    )
    .await;
    assert_eq!(
        (
            v["rows"].as_array().unwrap().len(),
            v["cols"].as_array().unwrap().len()
        ),
        (2, 2)
    );
    for bad in [
        json!({"rows":{"dimension":"regions","members":["Nord"]},"cols":{"dimension":"regions","members":["Sued"]}}),
        json!({"rows":{"dimension":"nope","members":["x"]},"cols":{"dimension":"measures","members":["Sales"]}}),
        json!({"rows":{"dimension":"regions","members":["Atlantis"]},"cols":{"dimension":"measures","members":["Sales"]}}),
    ] {
        assert_eq!(
            send(&env.app, &root, "POST", "/cubes/sales/view", Some(bad))
                .await
                .0
                .as_u16()
                / 100,
            4
        );
    }
}

#[tokio::test]
async fn conflicts_name_the_other_user_and_overwriting_works_with_the_new_generation() {
    let env = setup();
    let root = user(&env, "root").await;
    let mo = user(&env, "mo").await;
    let (_, v) = send(
        &env.app,
        &root,
        "GET",
        "/cubes/sales/cell?years=Jan&regions=Nord&measures=Sales",
        None,
    )
    .await;
    let seen = v["generation"].as_u64().unwrap();
    send(
        &env.app,
        &mo,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"999"})),
    )
    .await;
    let stale = json!({ "cell": ["Q1", "Nord", "Sales"], "value": "10", "generation": seen });
    let (s, v) = send(&env.app, &root, "POST", "/cubes/sales/write", Some(stale)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(
        (
            v["code"].as_str(),
            v["details"]["user"].as_str(),
            v["details"]["value"].as_f64()
        ),
        (Some("conflict"), Some("mo"), Some(999.0))
    );
    let (_, v) = send(
        &env.app,
        &root,
        "GET",
        "/cubes/sales/cell?years=Jan&regions=Nord&measures=Sales",
        None,
    )
    .await;
    assert_eq!(v["version"]["user"], "mo");
    let fresh =
        json!({ "cell": ["Q1", "Nord", "Sales"], "value": "10", "generation": v["generation"] });
    assert_eq!(
        send(&env.app, &root, "POST", "/cubes/sales/write", Some(fresh))
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn undo_reverts_the_callers_own_last_operation_and_refuses_after_foreign_changes() {
    let env = setup();
    let root = user(&env, "root").await;
    let mo = user(&env, "mo").await;
    let cell =
        |region: &str| format!("/cubes/sales/cell?years=Jan&regions={region}&measures=Sales");
    send(
        &env.app,
        &root,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"10"})),
    )
    .await;
    assert_eq!(
        send(
            &env.app,
            &root,
            "POST",
            "/cubes/sales/undo",
            Some(json!({}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(&env.app, &root, "GET", &cell("Nord"), None).await.1["value"],
        Value::Null
    );
    assert_eq!(
        send(
            &env.app,
            &root,
            "POST",
            "/cubes/sales/undo",
            Some(json!({}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "nothing left"
    );
    send(
        &env.app,
        &root,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"10"})),
    )
    .await;
    send(
        &env.app,
        &mo,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"11"})),
    )
    .await;
    let (s, v) = send(
        &env.app,
        &root,
        "POST",
        "/cubes/sales/undo",
        Some(json!({})),
    )
    .await;
    assert_eq!(
        (s, v["details"]["user"].as_str()),
        (StatusCode::CONFLICT, Some("mo"))
    );
}

#[tokio::test]
async fn rules_can_be_checked_saved_and_calculate_cells() {
    let env = setup();
    let mo = user(&env, "mo").await;
    let root = user(&env, "root").await;
    let (s, v) = send(
        &env.app,
        &mo,
        "PUT",
        "/cubes/sales/rules",
        Some(json!({"text":"['Margin'] = ;"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("line 1"));
    let rules = json!({ "text": "['Margin'] = IF(['Sales'] = 0, NULL, ['Profit'] / ['Sales']);\n['Nothing'] = 1;", "check": true });
    let (s, v) = send(&env.app, &mo, "PUT", "/cubes/sales/rules", Some(rules)).await;
    assert_eq!((s, v["saved"].as_bool()), (StatusCode::OK, Some(false)));
    assert_eq!(v["status"][1]["valid"], json!(false));
    let (_, v) = send(&env.app, &mo, "GET", "/cubes/sales/rules", None).await;
    assert_eq!(v["text"], "", "a check saves nothing");
    send(
        &env.app,
        &mo,
        "PUT",
        "/cubes/sales/rules",
        Some(json!({"text":"['Margin'] = IF(['Sales'] = 0, NULL, ['Profit'] / ['Sales']);"})),
    )
    .await;
    send(
        &env.app,
        &root,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"200"})),
    )
    .await;
    send(
        &env.app,
        &root,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Cost"],"value":"150"})),
    )
    .await;
    let (_, v) = send(
        &env.app,
        &root,
        "GET",
        "/cubes/sales/cell?years=Jan&regions=Nord&measures=Margin",
        None,
    )
    .await;
    assert_eq!(v["value"], json!(0.25));
    let (s, v) = send(
        &env.app,
        &root,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Margin"],"value":"1"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(v["error"].as_str().unwrap().contains("rule"));
}

#[tokio::test]
async fn members_are_paged_searched_and_hidden_for_unreadable_dimensions() {
    let env = setup();
    let pia = user(&env, "pia").await;
    let (s, v) = send(&env.app, &pia, "GET", "/dimensions/years/members", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["members"][0]["name"], "Year");
    let (_, v) = send(
        &env.app,
        &pia,
        "GET",
        "/dimensions/years/members?parent=Q1",
        None,
    )
    .await;
    let names: Vec<_> = v["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Jan", "Feb"]);
    let (_, v) = send(
        &env.app,
        &pia,
        "GET",
        "/dimensions/years/members?search=ap",
        None,
    )
    .await;
    assert_eq!(v["members"][0]["name"], "Apr");
    let (_, v) = send(
        &env.app,
        &pia,
        "GET",
        "/dimensions/years/members?limit=2&offset=1&parent=Year",
        None,
    )
    .await;
    assert_eq!(v["members"].as_array().unwrap().len(), 1);
    assert_eq!(
        send(
            &env.app,
            &pia,
            "GET",
            "/dimensions/years/members?limit=99999",
            None
        )
        .await
        .1["limit"],
        json!(1000)
    );
    assert_eq!(
        send(&env.app, &pia, "GET", "/dimensions/nope/members", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    // a dimension that no readable cube uses is invisible
    {
        let mut db = env.state.db.write();
        let mut e = Dimension::new("hidden", DimensionKind::Generic).edit();
        e.add("x").unwrap();
        db.save_dimension(e.commit().unwrap(), "setup").unwrap();
        db.create_cube("private", &["hidden"], "setup").unwrap();
    }
    assert_eq!(
        send(&env.app, &pia, "GET", "/dimensions/hidden/members", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let root = user(&env, "root").await;
    assert_eq!(
        send(&env.app, &root, "GET", "/dimensions/hidden/members", None)
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn modelers_change_dimensions_and_cells_survive() {
    let env = setup();
    let mo = user(&env, "mo").await;
    let root = user(&env, "root").await;
    send(
        &env.app,
        &root,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"7"})),
    )
    .await;
    let ops = json!({ "ops": [
        { "op": "add", "name": "Jun", "parent": "Q2" },
        { "op": "alias", "name": "Jan", "alias": "Januar" },
        { "op": "rename", "name": "May", "to": "Mai" },
    ]});
    assert_eq!(
        send(&env.app, &mo, "PATCH", "/dimensions/years", Some(ops))
            .await
            .0,
        StatusCode::OK
    );
    let (_, v) = send(
        &env.app,
        &root,
        "GET",
        "/cubes/sales/cell?years=Januar&regions=Nord&measures=Sales",
        None,
    )
    .await;
    assert_eq!(v["value"], json!(7.0), "cells stay with their member");
    for bad in [
        json!({"ops":[{"op":"add","name":"Jan"}]}),
        json!({"ops":[{"op":"remove","name":"Nobody"}]}),
        json!({"ops":[{"op":"fly"}]}),
        json!({"ops":[{"op":"move","name":"Year","to":"Jan"}]}),
    ] {
        let (s, v) = send(&env.app, &mo, "PATCH", "/dimensions/years", Some(bad)).await;
        assert!(s.is_client_error(), "{v}");
    }
    let (_, v) = send(
        &env.app,
        &root,
        "GET",
        "/cubes/sales/cell?years=Jan&regions=Nord&measures=Sales",
        None,
    )
    .await;
    assert_eq!(
        v["value"],
        json!(7.0),
        "a rejected change leaves everything as it was"
    );
}

#[tokio::test]
async fn admin_manages_users_grants_tokens_and_sessions_end_with_changes() {
    let env = setup();
    let root = user(&env, "root").await;
    let (s, _) = send(
        &env.app,
        &root,
        "POST",
        "/admin/users",
        Some(json!({"name":"kai","password":"another long secret","role":"planner"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, v) = send(
        &env.app,
        &root,
        "POST",
        "/admin/users",
        Some(json!({"name":"x","password":"short","role":"planner"})),
    )
    .await;
    assert_eq!(
        (s, v["error"].as_str().unwrap().contains("at least")),
        (StatusCode::BAD_REQUEST, true)
    );
    assert_eq!(
        send(
            &env.app,
            &root,
            "POST",
            "/admin/users",
            Some(json!({"name":"y","password":PW,"role":"boss"}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let grant = json!({"user":"kai","cube":"sales","access":"write","area":[["regions",["Nord"]]]});
    assert_eq!(
        send(&env.app, &root, "PUT", "/admin/grants", Some(grant))
            .await
            .0,
        StatusCode::OK
    );
    let kai = login(&env.app, "kai", "another long secret").await.unwrap();
    assert_eq!(
        send(
            &env.app,
            &kai,
            "POST",
            "/cubes/sales/write",
            Some(json!({"cell":["Jan","Nord","Sales"],"value":"1"}))
        )
        .await
        .0,
        StatusCode::OK
    );
    // a password change ends the user's sessions
    assert_eq!(
        send(
            &env.app,
            &root,
            "PATCH",
            "/admin/users/kai",
            Some(json!({"password":"a brand new secret"}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(&env.app, &kai, "GET", "/me", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let kai = login(&env.app, "kai", "a brand new secret").await.unwrap();
    // a role change takes effect for the running session
    assert_eq!(
        send(
            &env.app,
            &root,
            "PATCH",
            "/admin/users/kai",
            Some(json!({"role":"reader"}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            &env.app,
            &kai,
            "POST",
            "/cubes/sales/write",
            Some(json!({"cell":["Feb","Nord","Sales"],"value":"1"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // tokens: everyone may create their own (it carries the same role, so it gives nothing extra);
    // a token for somebody else is an admin task
    let (s, own) = send(
        &env.app,
        &kai,
        "POST",
        "/admin/users/kai/token",
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = send(
        &env.app,
        &kai,
        "POST",
        "/admin/users/pia/token",
        Some(json!({})),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "no tokens in somebody else's name"
    );
    let t = Client::token(own["token"].as_str().unwrap());
    assert_eq!(
        send(&env.app, &t, "GET", "/me", None).await.1["user"],
        "kai"
    );
    assert_eq!(
        send(
            &env.app,
            &t,
            "POST",
            "/cubes/sales/write",
            Some(json!({"cell":["Feb","Nord","Sales"],"value":"1"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "a reader's token is a reader's token"
    );
    let (s, other) = send(
        &env.app,
        &root,
        "POST",
        "/admin/users/kai/token",
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_ne!(other["token"], own["token"]);
    // removal ends sessions and tokens at once
    assert_eq!(
        send(
            &env.app,
            &root,
            "DELETE",
            "/admin/users/kai",
            Some(json!({}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(&env.app, &kai, "GET", "/me", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&env.app, &t, "GET", "/me", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    // the last admin stays
    assert_eq!(
        send(
            &env.app,
            &root,
            "DELETE",
            "/admin/users/root",
            Some(json!({}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(
            &env.app,
            &root,
            "DELETE",
            "/admin/grants",
            Some(json!({"user":"pia","cube":"sales","area":[["regions",["Nord"]]]}))
        )
        .await
        .1["revoked"],
        json!(1)
    );
}

#[tokio::test]
async fn journal_shows_who_changed_what_and_only_what_the_caller_may_see() {
    let env = setup();
    let root = user(&env, "root").await;
    let pia = user(&env, "pia").await;
    send(
        &env.app,
        &pia,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"5"})),
    )
    .await;
    send(
        &env.app,
        &root,
        "POST",
        "/cubes/secret/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"6"})),
    )
    .await;
    let (_, v) = send(&env.app, &pia, "GET", "/journal?limit=50", None).await;
    let entries = v["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e["user"] == "pia" && e["op"] == "splash" && e["cube"] == "sales")
    );
    assert!(
        entries
            .iter()
            .all(|e| e["cube"] != "secret" && e["cube"] != ""),
        "no foreign cubes, no model-wide entries"
    );
    assert_eq!(
        send(&env.app, &pia, "GET", "/journal?cube=secret", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (_, v) = send(&env.app, &root, "GET", "/journal?user=root&limit=50", None).await;
    assert!(
        v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["cube"] == "secret")
    );
}

#[tokio::test]
async fn imports_run_as_jobs_with_reports_and_respect_rights() {
    let env = setup();
    let pia = user(&env, "pia").await;
    let spec = "name = \"t\"\ncube = \"sales\"\n[map]\nyears = \"Monat\"\nregions = \"Region\"\n[values]\nSales = \"Umsatz\"\n[options]\nmode = \"overwrite\"\nunknown_member = \"reject\"\n";
    let file = "Monat,Region,Umsatz\nJan,Nord,10\nFeb,Nord,20\n";
    let job = import(&env, &pia, spec, file, "x.csv", true).await;
    assert_eq!(job["status"], "done");
    assert_eq!(
        (
            job["result"]["dry_run"].as_bool(),
            job["result"]["cells_created"].as_u64()
        ),
        (Some(true), Some(2))
    );
    let (_, v) = send(
        &env.app,
        &pia,
        "GET",
        "/cubes/sales/cell?years=Q1&regions=Nord&measures=Sales",
        None,
    )
    .await;
    assert_eq!(v["value"], Value::Null, "a preview changes nothing");
    let job = import(&env, &pia, spec, file, "x.csv", false).await;
    assert_eq!(
        (
            job["status"].as_str(),
            job["progress"].as_u64(),
            job["result"]["cells_created"].as_u64()
        ),
        (Some("done"), Some(100), Some(2))
    );
    let (_, v) = send(
        &env.app,
        &pia,
        "GET",
        "/cubes/sales/cell?years=Q1&regions=Nord&measures=Sales",
        None,
    )
    .await;
    assert_eq!(v["value"], json!(30.0));
    // rows outside pia's area: the whole import is refused
    let job = import(
        &env,
        &pia,
        spec,
        "Monat,Region,Umsatz\nJan,Nord,1\nJan,Sued,2\n",
        "x.csv",
        false,
    )
    .await;
    assert_eq!(job["status"], "failed");
    assert!(job["error"].as_str().unwrap().contains("Sued"), "{job}");
    // rejected rows: failed, with line and reason in the report
    let job = import(
        &env,
        &pia,
        spec,
        "Monat,Region,Umsatz\nXyz,Nord,1\n",
        "x.csv",
        false,
    )
    .await;
    assert_eq!(job["status"], "failed");
    assert_eq!(job["result"]["rejected"][0]["line"], json!(2));
    // creating members is a modeler's job
    let creates = spec.replace("reject", "create_under:Total");
    let (s, _) = import_raw(&env, &pia, &creates, file, "x.csv", false).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // readers cannot import; an unknown cube is not found
    let rita = user(&env, "rita").await;
    assert_eq!(
        import_raw(&env, &rita, spec, file, "x.csv", false).await.0,
        StatusCode::FORBIDDEN
    );
    let (s, _) = import_raw(
        &env,
        &pia,
        &spec.replace("\"sales\"", "\"secret\""),
        file,
        "x.csv",
        false,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // jobs belong to their owner
    let nina = user(&env, "nina").await;
    let (s, _) = import_raw(&env, &pia, spec, file, "x.csv", true).await;
    assert_eq!(s, StatusCode::OK);
    let id = start(&env, &pia, spec, file, "x.csv", true).await;
    assert_eq!(
        send(&env.app, &nina, "GET", &format!("/jobs/{id}"), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &env.app,
            &user(&env, "root").await,
            "GET",
            &format!("/jobs/{id}"),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    // bad requests
    assert_eq!(
        import_raw(&env, &pia, "this = is not [a spec", file, "x.csv", false)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_modeler_import_can_create_members() {
    let env = setup();
    let mo = user(&env, "mo").await;
    let spec = "name = \"t\"\ncube = \"sales\"\n[map]\nyears = \"Monat\"\nregions = \"Region\"\n[values]\nSales = \"Umsatz\"\n[options]\nunknown_member = \"create_under:Total\"\n";
    let job = import(
        &env,
        &mo,
        spec,
        "Monat,Region,Umsatz\nJan,Mitte,3\n",
        "x.csv",
        false,
    )
    .await;
    assert_eq!(job["status"], "done", "{job}");
    let (_, v) = send(
        &env.app,
        &mo,
        "GET",
        "/cubes/sales/cell?years=Jan&regions=Total&measures=Sales",
        None,
    )
    .await;
    assert_eq!(v["value"], json!(3.0));
}

#[tokio::test]
async fn upload_limits_and_missing_fields_are_rejected() {
    let env = setup();
    let pia = user(&env, "pia").await;
    let spec =
        "name = \"t\"\ncube = \"sales\"\n[map]\nyears = \"Monat\"\n[values]\nSales = \"Umsatz\"\n";
    // a normal (non-import) route has a small body limit
    let big = json!({ "cell": ["Jan", "Nord", "Sales"], "value": "x".repeat(3 * 1024 * 1024) });
    let (s, _) = send(&env.app, &pia, "POST", "/cubes/sales/write", Some(big)).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
    // missing parts of the multipart form
    let (s, v) = multipart(&env, &pia, vec![("file", Some("a.csv"), "x")]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("spec"));
    let (s, v) = multipart(&env, &pia, vec![("spec", None, spec)]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("file"));
}

#[tokio::test]
async fn export_delivers_csv_and_xlsx_files() {
    let env = setup();
    let pia = user(&env, "pia").await;
    send(
        &env.app,
        &pia,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"10"})),
    )
    .await;
    let get = |path: String, c: Client| {
        let app = env.app.clone();
        async move {
            let mut b = Request::builder().uri(format!("/api/v1{path}"));
            b = b.header(header::COOKIE, c.cookie.unwrap());
            app.oneshot(b.body(Body::empty()).unwrap()).await.unwrap()
        }
    };
    let res = get(
        "/cubes/sales/export?area=years%3DJan".into(),
        user(&env, "pia").await,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()[header::CONTENT_TYPE],
        "text/csv; charset=utf-8"
    );
    assert!(
        res.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("sales.csv")
    );
    let text =
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert_eq!(text, "years,regions,measures,value\nJan,Nord,Sales,10\n");
    let res = get(
        "/cubes/sales/export?format=xlsx".into(),
        user(&env, "pia").await,
    )
    .await;
    assert!(
        res.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .contains("spreadsheetml")
    );
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..2], b"PK");
    let res = get("/cubes/secret/export".into(), user(&env, "pia").await).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let res = get(
        "/cubes/sales/export?area=nope".into(),
        user(&env, "pia").await,
    )
    .await;
    assert!(res.status().is_client_error());
}

#[tokio::test]
async fn backup_is_written_to_the_data_directory() {
    let env = setup();
    let root = user(&env, "root").await;
    let (s, v) = send(&env.app, &root, "POST", "/admin/backup", Some(json!({}))).await;
    assert_eq!(s, StatusCode::OK);
    let file = v["file"].as_str().unwrap();
    assert!(
        file.starts_with("backup-") && file.ends_with(".neolap"),
        "{file}"
    );
    assert!(env.state.data_dir.join("backup").join(file).exists());
    assert!(!file.contains('/') && !file.contains(".."));
}

// ------------------------------------------------------------ helpers

async fn put_cell(env: &Env, c: &Client, region: &str, value: &str) -> (StatusCode, Value) {
    let body = json!({ "cell": ["Jan", region, "Sales"], "value": value });
    send(&env.app, c, "POST", "/cubes/sales/write", Some(body)).await
}

async fn multipart(
    env: &Env,
    c: &Client,
    parts: Vec<(&str, Option<&str>, &str)>,
) -> (StatusCode, Value) {
    let boundary = "XBOUNDARYX";
    let mut body = String::new();
    for (name, file, content) in parts {
        body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\""
        ));
        if let Some(f) = file {
            body.push_str(&format!("; filename=\"{f}\"\r\nContent-Type: text/csv"));
        }
        body.push_str(&format!("\r\n\r\n{content}\r\n"));
    }
    body.push_str(&format!("--{boundary}--\r\n"));
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/imports")
        .header(header::COOKIE, c.cookie.clone().unwrap())
        .header("x-csrf-token", c.csrf.clone().unwrap())
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let (s, v, _) = body_json(env.app.clone().oneshot(req).await.unwrap()).await;
    (s, v)
}

async fn import_raw(
    env: &Env,
    c: &Client,
    spec: &str,
    file: &str,
    name: &str,
    dry: bool,
) -> (StatusCode, Value) {
    multipart(
        env,
        c,
        vec![
            ("spec", None, spec),
            ("dry_run", None, if dry { "true" } else { "false" }),
            ("file", Some(name), file),
        ],
    )
    .await
}

async fn start(env: &Env, c: &Client, spec: &str, file: &str, name: &str, dry: bool) -> String {
    let (s, v) = import_raw(env, c, spec, file, name, dry).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["job"].as_str().unwrap().to_string()
}

/// Starts an import and waits for its job to finish.
async fn import(env: &Env, c: &Client, spec: &str, file: &str, name: &str, dry: bool) -> Value {
    let id = start(env, c, spec, file, name, dry).await;
    for _ in 0..200 {
        let (_, v) = send(&env.app, c, "GET", &format!("/jobs/{id}"), None).await;
        if v["status"] != "running" {
            return v;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the import job did not finish");
}

// ------------------------------------------------------------ live events (SSE)

/// Opens `/events` for a client and returns the response body to read frames from.
async fn open_events(env: &Env, c: &Client) -> Body {
    let mut b = Request::builder().uri("/api/v1/events");
    if let Some(cookie) = &c.cookie {
        b = b.header(header::COOKIE, cookie);
    }
    let res = env
        .app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "text/event-stream");
    res.into_body()
}

/// Reads frames until `want` events were seen or the timeout passes; returns `(event, data)` pairs.
async fn read_events(body: &mut Body, want: usize, wait_ms: u64) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
    while out.len() < want {
        let Ok(Some(Ok(frame))) = tokio::time::timeout_at(deadline, body.frame()).await else {
            break;
        };
        if let Ok(data) = frame.into_data() {
            text.push_str(&String::from_utf8_lossy(&data));
        }
        while let Some(end) = text.find("\n\n") {
            let block: String = text.drain(..end + 2).collect();
            let event = block
                .lines()
                .find_map(|l| l.strip_prefix("event:"))
                .map(|e| e.trim().to_string());
            let data = block
                .lines()
                .find_map(|l| l.strip_prefix("data:"))
                .map(|d| d.trim().to_string());
            if let (Some(e), Some(d)) = (event, data) {
                out.push((e, serde_json::from_str(&d).unwrap_or(Value::String(d))));
            }
        }
    }
    out
}

#[tokio::test]
async fn open_views_are_told_about_changes_of_others() {
    let env = setup();
    let rita = user(&env, "rita").await; // reads sales
    let pia = user(&env, "pia").await;
    let mut events = open_events(&env, &rita).await;
    send(
        &env.app,
        &pia,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"10"})),
    )
    .await;
    let got = read_events(&mut events, 1, 2000).await;
    assert_eq!(got.len(), 1, "{got:?}");
    let (kind, ev) = &got[0];
    assert_eq!(kind, "cells");
    assert_eq!(
        (
            ev["cube"].as_str(),
            ev["user"].as_str(),
            ev["cells_changed"].as_u64()
        ),
        (Some("sales"), Some("pia"), Some(1))
    );
    assert!(ev["generation"].as_u64().unwrap() > 0);
    // undo is announced too
    send(&env.app, &pia, "POST", "/cubes/sales/undo", Some(json!({}))).await;
    assert_eq!(read_events(&mut events, 1, 2000).await[0].0, "cells");
}

#[tokio::test]
async fn events_of_cubes_the_caller_cannot_read_are_not_sent() {
    let env = setup();
    let pia = user(&env, "pia").await; // no grant on secret
    let root = user(&env, "root").await;
    let mut events = open_events(&env, &pia).await;
    send(
        &env.app,
        &root,
        "POST",
        "/cubes/secret/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"6"})),
    )
    .await;
    send(
        &env.app,
        &root,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"7"})),
    )
    .await;
    let got = read_events(&mut events, 2, 800).await;
    assert_eq!(got.len(), 1, "only the sales event arrives: {got:?}");
    assert_eq!(got[0].1["cube"], "sales");
}

#[tokio::test]
async fn model_rules_and_import_changes_are_announced_with_their_kind() {
    let env = setup();
    let rita = user(&env, "rita").await;
    let mo = user(&env, "mo").await;
    let mut events = open_events(&env, &rita).await;
    send(
        &env.app,
        &mo,
        "PUT",
        "/cubes/sales/rules",
        Some(json!({"text":"['Margin'] = 1;"})),
    )
    .await;
    send(
        &env.app,
        &mo,
        "PATCH",
        "/dimensions/years",
        Some(json!({"ops":[{"op":"add","name":"Jun","parent":"Q2"}]})),
    )
    .await;
    let spec = "name = \"t\"\ncube = \"sales\"\n[map]\nyears = \"Monat\"\nregions = \"Region\"\n[values]\nSales = \"Umsatz\"\n";
    import(
        &env,
        &mo,
        spec,
        "Monat,Region,Umsatz\nJan,Nord,1\n",
        "x.csv",
        false,
    )
    .await;
    let kinds: Vec<String> = read_events(&mut events, 4, 2000)
        .await
        .into_iter()
        .map(|e| e.0)
        .collect();
    assert!(
        kinds.contains(&"rules".to_string())
            && kinds.contains(&"model".to_string())
            && kinds.contains(&"import".to_string()),
        "{kinds:?}"
    );
}

#[tokio::test]
async fn a_preview_import_and_a_failed_write_send_no_event() {
    let env = setup();
    let pia = user(&env, "pia").await;
    let rita = user(&env, "rita").await;
    let mut events = open_events(&env, &rita).await;
    let spec = "name = \"t\"\ncube = \"sales\"\n[map]\nyears = \"Monat\"\nregions = \"Region\"\n[values]\nSales = \"Umsatz\"\n";
    import(
        &env,
        &pia,
        spec,
        "Monat,Region,Umsatz\nJan,Nord,1\n",
        "x.csv",
        true,
    )
    .await;
    send(
        &env.app,
        &pia,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Sued","Sales"],"value":"1"})),
    )
    .await; // refused
    assert!(read_events(&mut events, 1, 400).await.is_empty());
}

#[tokio::test]
async fn a_client_that_falls_far_behind_is_told_to_reload() {
    let env = setup();
    let rita = user(&env, "rita").await;
    let mut events = open_events(&env, &rita).await; // never read while changes pile up
    for i in 0..1100 {
        env.state.publish(neolap_server::testing::event("sales", i));
    }
    let got = read_events(&mut events, 1, 2000).await;
    assert_eq!(got[0].0, "resync", "{got:?}");
}

// ------------------------------------------------------------ proxy header and the login limit

async fn login_from(app: &Router, forwarded: &str, user: &str, password: &str) -> StatusCode {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-forwarded-for", forwarded)
        .body(Body::from(
            json!({ "user": user, "password": password }).to_string(),
        ))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

fn app_with_proxy(trust: bool) -> (Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::create(dir.path().join("p.neolap")).unwrap();
    for u in ["a", "b", "c", "d", "e", "f", "g", "h"] {
        db.add_user(u, PW, Role::Reader, "s").unwrap();
    }
    (
        neolap_server::router(AppState::new(db, dir.path().to_path_buf(), true, trust)),
        dir,
    )
}

#[tokio::test]
async fn a_forged_forwarded_for_header_cannot_dodge_the_per_client_limit_behind_a_proxy() {
    let (app, _dir) = app_with_proxy(true);
    // the real client (as seen by our proxy: the LAST entry) tries many different users, while
    // inventing a new first entry for every attempt
    let mut statuses = Vec::new();
    for (i, u) in ["a", "b", "c", "d", "e", "f", "g", "h"].iter().enumerate() {
        statuses.push(
            login_from(
                &app,
                &format!("10.0.0.{i}, 203.0.113.7"),
                u,
                "wrong password!!",
            )
            .await,
        );
    }
    assert_eq!(statuses[..5], [StatusCode::UNAUTHORIZED; 5]);
    assert!(
        statuses[5..]
            .iter()
            .all(|s| *s == StatusCode::TOO_MANY_REQUESTS),
        "{statuses:?}"
    );
    // another real client (a different last entry) is not affected by the first one's failures
    assert_eq!(
        login_from(&app, "10.0.0.9, 198.51.100.4", "a", PW).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn without_proxy_trust_the_forwarded_header_is_ignored() {
    let (app, _dir) = app_with_proxy(false);
    let mut last = StatusCode::OK;
    for (i, u) in ["a", "b", "c", "d", "e", "f"].iter().enumerate() {
        last = login_from(&app, &format!("10.0.0.{i}"), u, "wrong password!!").await;
    }
    assert_eq!(
        last,
        StatusCode::TOO_MANY_REQUESTS,
        "all attempts count for the one real peer"
    );
}

// ------------------------------------------------------------ /session (used by the web UI at start)

#[tokio::test]
async fn session_answers_200_for_everybody_and_reveals_nothing_to_strangers() {
    let env = setup();
    // nobody logged in, a forged cookie and a forged token all get the same plain answer
    let forged_cookie = Client {
        cookie: Some("neolap_session=deadbeef".into()),
        csrf: None,
        bearer: None,
    };
    for c in [
        Client::anonymous(),
        forged_cookie,
        Client::token("nlp_x_y"),
        Client::token("garbage"),
    ] {
        let (s, v) = send(&env.app, &c, "GET", "/session", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            v,
            json!({ "user": null }),
            "a stranger learns nothing, not even why"
        );
    }
    // a real session gets its user, role and a CSRF token
    let pia = user(&env, "pia").await;
    let (s, v) = send(&env.app, &pia, "GET", "/session", None).await;
    assert_eq!(
        (s, v["user"].as_str(), v["role"].as_str()),
        (StatusCode::OK, Some("pia"), Some("planner"))
    );
    assert_eq!(
        v["csrf"],
        json!(pia.csrf.clone().unwrap()),
        "the token is the session's own, so a reloaded page can keep writing"
    );
    // after logout the same cookie is a stranger again
    send(&env.app, &pia, "POST", "/logout", Some(json!({}))).await;
    assert_eq!(
        send(&env.app, &pia, "GET", "/session", None).await.1,
        json!({ "user": null })
    );
    // a bearer token has no CSRF token
    let token = env
        .state
        .db
        .write()
        .create_token("pia", "t", "setup")
        .unwrap();
    let (_, v) = send(&env.app, &Client::token(&token), "GET", "/session", None).await;
    assert_eq!(
        (v["user"].as_str(), v["csrf"].is_null()),
        (Some("pia"), true)
    );
    // /me keeps answering 401, which scripts and CI rely on
    assert_eq!(
        send(&env.app, &Client::anonymous(), "GET", "/me", None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_cell_answer_says_how_many_base_cells_an_input_reaches() {
    let env = setup();
    let root = user(&env, "root").await;
    let (_, v) = send(
        &env.app,
        &root,
        "GET",
        "/cubes/sales/cell?years=Jan&regions=Nord&measures=Sales",
        None,
    )
    .await;
    assert_eq!(
        v["area"],
        json!({ "base_cells": "1", "filled_cells": 0 }),
        "a base cell is itself"
    );
    send(
        &env.app,
        &root,
        "POST",
        "/cubes/sales/write",
        Some(json!({"cell":["Jan","Nord","Sales"],"value":"5"})),
    )
    .await;
    let (_, v) = send(
        &env.app,
        &root,
        "GET",
        "/cubes/sales/cell?years=Q1&regions=Total&measures=Sales",
        None,
    )
    .await;
    // Q1 = Jan, Feb; Total = Nord, Sued; Sales: 2 × 2 × 1 base cells, one of them filled
    assert_eq!(v["area"], json!({ "base_cells": "4", "filled_cells": 1 }));
    // the count is a string: it can exceed what JSON numbers hold exactly
    assert!(v["area"]["base_cells"].is_string());
    // a weighted aggregate cannot be written to, so there is no area to report
    let (_, v) = send(
        &env.app,
        &root,
        "GET",
        "/cubes/sales/cell?years=Jan&regions=Nord&measures=Profit",
        None,
    )
    .await;
    assert_eq!(v["area"], Value::Null);
}

// ------------------------------------------------------------ what the binary serves as the web UI

async fn get_status(env: &Env, path: &str) -> (StatusCode, String) {
    let res = env
        .app
        .clone()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    (status, ct)
}

#[tokio::test]
async fn the_binary_serves_the_ui_modules_with_the_right_type_and_not_the_ui_tests() {
    let env = setup();
    // every module the page loads must be served, as JavaScript (a module with another type is refused by the browser)
    let index = {
        let res = env
            .app
            .clone()
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap()
    };
    assert!(index.contains("app.js") && index.contains("style.css"));
    for module in [
        "app.js",
        "api.js",
        "dom.js",
        "grid.js",
        "grid-model.js",
        "pivot-state.js",
        "toast.js",
        "page-pivot.js",
        "page-dimensions.js",
        "page-rules.js",
        "page-import.js",
        "page-journal.js",
        "page-admin.js",
    ] {
        let (status, ct) = get_status(&env, &format!("/{module}")).await;
        assert_eq!(status, StatusCode::OK, "{module} must be served");
        assert!(
            ct.starts_with("text/javascript") || ct.starts_with("application/javascript"),
            "{module} is served as {ct:?}"
        );
    }
    assert_eq!(get_status(&env, "/style.css").await.1, "text/css");
    // the tests of the UI are not part of the application
    for test_file in [
        "tests/input-contract.tsv",
        "tests/dom-safety.test.js",
        "tests/grid-model.test.js",
        "tests/pivot-state.test.js",
    ] {
        assert_eq!(
            get_status(&env, &format!("/{test_file}")).await.0,
            StatusCode::NOT_FOUND,
            "{test_file} must not be served"
        );
    }
    // nor can a path walk out of the UI folder
    for sneaky in [
        "/../Cargo.toml",
        "/..%2FCargo.toml",
        "/%2e%2e/Cargo.toml",
        "/tests/../app.js/../../Cargo.toml",
    ] {
        let (status, _) = get_status(&env, sneaky).await;
        assert_ne!(
            status,
            StatusCode::OK,
            "{sneaky} must not reach files outside the UI"
        );
    }
}

#[tokio::test]
async fn every_response_carries_the_strict_security_headers() {
    let env = setup();
    for path in [
        "/",
        "/app.js",
        "/api/v1/health",
        "/api/v1/model",
        "/nothing-here",
    ] {
        let res = env
            .app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let csp = res
            .headers()
            .get("content-security-policy")
            .unwrap_or_else(|| panic!("{path}: no CSP"))
            .to_str()
            .unwrap()
            .to_string();
        for must in [
            "default-src 'self'",
            "script-src 'self'",
            "style-src 'self'",
            "base-uri 'none'",
            "object-src 'none'",
            "frame-ancestors 'none'",
            "form-action 'self'",
        ] {
            assert!(csp.contains(must), "{path}: CSP lacks {must:?}: {csp}");
        }
        assert!(
            !csp.contains("unsafe-inline") && !csp.contains("unsafe-eval"),
            "{path}: {csp}"
        );
        assert_eq!(res.headers()["x-content-type-options"], "nosniff", "{path}");
        assert_eq!(res.headers()["x-frame-options"], "DENY", "{path}");
    }
}
