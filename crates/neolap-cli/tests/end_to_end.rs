//! End-to-end test of the command line: the real `neolap` binary on a database file.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn neolap() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_neolap"));
    c.env("NEOLAP_USER", "tester").env_remove("NEOLAP_PASSWORD");
    c
}

fn ok(args: &[&str]) -> String {
    let o = neolap().args(args).output().unwrap();
    assert!(
        o.status.success(),
        "neolap {args:?} failed:\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap()
}

fn fails(args: &[&str]) -> String {
    let o: Output = neolap().args(args).output().unwrap();
    assert!(
        !o.status.success(),
        "neolap {args:?} should have failed:\n{}",
        String::from_utf8_lossy(&o.stdout)
    );
    String::from_utf8(o.stderr).unwrap()
}

fn p(dir: &Path, name: &str) -> String {
    dir.join(name).display().to_string()
}

fn value(db: &str, cells: &[&str]) -> f64 {
    let mut args = vec!["get", db, "sales"];
    args.extend(cells);
    ok(&args).trim().parse().unwrap()
}

#[test]
fn the_whole_chapter_8_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let db = p(d, "plan.neolap");

    // demo, info
    ok(&["demo", "tiny", &db]);
    let info = ok(&["info", &db]);
    assert!(
        info.contains("cube") && info.contains("sales") && info.contains("4 base members"),
        "{info}"
    );
    assert!(fails(&["demo", "tiny", &db]).contains("already exists"));
    assert!(fails(&["demo", "huge", &p(d, "x.neolap")]).contains("unknown demo"));

    // get: base cell and aggregate
    let jan = value(&db, &["Jan", "North", "sedan", "Sales"]);
    assert_eq!(jan, 1000.0);
    let q1 = value(&db, &["Q1", "North", "sedan", "Sales"]);
    assert!(q1 > jan);
    assert_eq!(
        ok(&[
            "get", &db, "sales", "Jan", "North", "sedan", "Sales", "--json"
        ])
        .trim(),
        "1000.0"
    );

    // get with rules: the demo model's margin rule
    let m = ok(&[
        "get",
        &db,
        "sales",
        "Year",
        "Total",
        "All",
        "Profit in %",
        "--rules",
    ]);
    let margin: f64 = m.trim().parse().unwrap();
    assert!((margin - 0.35).abs() < 1e-9, "{margin}");

    // set: a base cell, then a splash that hits the target exactly and keeps proportions
    ok(&[
        "set", &db, "sales", "Jan", "North", "sedan", "Sales", "2000",
    ]);
    assert_eq!(value(&db, &["Jan", "North", "sedan", "Sales"]), 2000.0);
    ok(&["set", &db, "sales", "Q1", "Total", "All", "Sales", "100000"]);
    assert!((value(&db, &["Q1", "Total", "All", "Sales"]) - 100000.0).abs() < 1e-6);
    ok(&[
        "set", &db, "sales", "Q1", "Total", "All", "Sales", "--mode", "equal", "24000",
    ]);
    assert_eq!(
        value(&db, &["Jan", "North", "sedan", "Sales"]),
        24000.0 / 24.0
    );
    ok(&["set", &db, "sales", "Q1", "Total", "All", "Sales", "+10%"]);
    assert!((value(&db, &["Q1", "Total", "All", "Sales"]) - 26400.0).abs() < 1e-6);
    // refused: rule cell, wrong number of members, bad input
    assert!(
        fails(&[
            "set",
            &db,
            "sales",
            "Jan",
            "North",
            "sedan",
            "Profit in %",
            "1"
        ])
        .contains("rule in line")
    );
    assert!(fails(&["set", &db, "sales", "Jan", "5"]).contains("expected 4 members"));
    assert!(
        fails(&["set", &db, "sales", "Jan", "North", "sedan", "Sales", "abc"])
            .contains("not a number")
    );
    assert!(
        fails(&["set", &db, "sales", "Jan", "North", "nothing", "Sales", "1"]).contains("nothing")
    );
    assert!(fails(&["get", &db, "nocube", "x"]).contains("cube 'nocube' not found"));

    // export: long CSV to stdout, wide xlsx and csv to files
    let csv = ok(&[
        "export",
        &db,
        "sales",
        "--area",
        "years=Jan|Feb,regions=North,products=sedan,measures=Sales",
    ]);
    assert_eq!(csv.lines().count(), 3, "{csv}");
    assert!(csv.starts_with("years;regions;products;measures;value\n"));
    let wide = p(d, "wide.csv");
    ok(&[
        "export",
        &db,
        "sales",
        "--area",
        "years=Q1,regions=Total,products=All",
        "--aggregates",
        "--wide",
        "measures",
        "--decimal",
        ",",
        "-o",
        &wide,
    ]);
    let wide_text = std::fs::read_to_string(&wide).unwrap();
    // columns follow the dimension: base members in tree order, then aggregates; without --rules
    // the rule measure 'Profit in %' has no stored values, so its cell is empty
    assert_eq!(
        wide_text,
        "years;regions;products;Sales;Cost;Profit in %;Profit\nQ1;Total;All;26400;21996;;4404\n"
    );
    let with_rules = ok(&[
        "export",
        &db,
        "sales",
        "--area",
        "years=Q1,regions=Total,products=All",
        "--aggregates",
        "--wide",
        "measures",
        "--rules",
    ]);
    let row: Vec<&str> = with_rules.lines().nth(1).unwrap().split(';').collect();
    let margin: f64 = row[5].parse().unwrap();
    assert!((margin - 4404.0 / 26400.0).abs() < 1e-9, "{with_rules}");
    let xlsx = p(d, "out.xlsx");
    ok(&[
        "export",
        &db,
        "sales",
        "--area",
        "years=Jan,regions=North",
        "-o",
        &xlsx,
    ]);
    assert!(std::fs::metadata(&xlsx).unwrap().len() > 1000);
    assert!(fails(&["export", &db, "sales", "--area", "years"]).contains("dimension=member"));

    // load: export long, wipe the area by loading a changed file with replace_area, dry run first
    let long = p(d, "long.csv");
    ok(&[
        "export",
        &db,
        "sales",
        "--area",
        "years=Jan|Feb|Mar",
        "-o",
        &long,
        "--delimiter",
        ",",
    ]);
    let spec = d.join("laden.toml");
    std::fs::write(
        &spec,
        "name = \"zurueck\"\ncube = \"sales\"\nvalue_column = \"value\"\nsource = { file = \"long.csv\", delimiter = \",\" }\n[map]\nyears = \"years\"\nregions = \"regions\"\nproducts = \"products\"\nmeasures = \"measures\"\n[options]\nmode = \"replace_area\"\n",
    )
    .unwrap();
    ok(&["set", &db, "sales", "Jan", "North", "sedan", "Sales", "1"]);
    let dry = ok(&[
        "load",
        &db,
        &spec.display().to_string(),
        "--dry-run",
        "--json",
    ]);
    assert!(
        dry.contains("\"dry_run\":\"true\"") && dry.contains("\"cells_updated\":1.0"),
        "{dry}"
    );
    assert_eq!(value(&db, &["Jan", "North", "sedan", "Sales"]), 1.0); // dry run changed nothing
    let real = ok(&["load", &db, &spec.display().to_string(), "--json"]);
    assert!(real.contains("\"cells_updated\":1.0"), "{real}");
    // the file was exported after "equal 24000" (1000 per cell) and "+10%": back to 1100
    assert_eq!(value(&db, &["Jan", "North", "sedan", "Sales"]), 1100.0);
    let again = ok(&["load", &db, &spec.display().to_string(), "--json"]);
    assert!(
        again.contains("\"cells_updated\":0.0") && again.contains("\"cells_created\":0.0"),
        "replace_area twice = once: {again}"
    );
    // rejected rows are reported with line and reason, exit code 1, nothing written
    std::fs::write(
        d.join("long.csv"),
        "years,regions,products,measures,value\nJan,North,sedan,Sales,5\nXyz,North,sedan,Sales,5\n",
    )
    .unwrap();
    let err = fails(&["load", &db, &spec.display().to_string()]);
    assert!(
        err.contains("line 3") && err.contains("Xyz") && err.contains("nothing was loaded"),
        "{err}"
    );
    assert!(fails(&["load", &db, "nothing.toml"]).contains("neither a file"));

    // rules
    let rules = d.join("sales.rules");
    // a rule naming an unknown member is invalid (it would be skipped): the check fails and says which
    std::fs::write(
        &rules,
        "['Profit in %'] = IF(['Sales'] = 0, NULL, ['Profit'] / ['Sales']);\n['Nothing'] = 1;\n",
    )
    .unwrap();
    let rules_path = rules.display().to_string();
    let o = neolap()
        .args(["rules", "check", &db, "sales", &rules_path])
        .output()
        .unwrap();
    assert!(!o.status.success());
    let (stdout, stderr) = (
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr),
    );
    assert!(stderr.contains("1 rule(s) invalid"), "{stderr}");
    assert!(
        stdout.contains("invalid") && stdout.contains("Nothing"),
        "{stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with('1') && l.contains("ok")),
        "{stdout}"
    );
    std::fs::write(&rules, "['Profit in %'] = ['Profit'] / ['Sales'] * 100;\n").unwrap();
    assert!(ok(&["rules", "check", &db, "sales", &rules.display().to_string()]).contains("ok"));
    ok(&["rules", "apply", &db, "sales", &rules.display().to_string()]);
    assert!(ok(&["rules", "show", &db, "sales"]).contains("* 100"));
    std::fs::write(&rules, "['Profit in %'] = ;").unwrap();
    assert!(
        fails(&["rules", "apply", &db, "sales", &rules.display().to_string()]).contains("line 1")
    );
    assert!(
        ok(&["rules", "show", &db, "sales"]).contains("* 100"),
        "a rejected text changes nothing"
    );

    // dimension import: parent-child with attribute, then levels with --sync
    let regions = d.join("regions.csv");
    std::fs::write(&regions, "child;parent;weight;alias;currency\nNorth;Total;;;EUR\nSouth;Total;;;CHF\nEast;Total\nWest;Total\nCentral;Total;;Mitte;EUR\n").unwrap();
    let r = ok(&[
        "dim",
        "import",
        &db,
        "regions",
        &regions.display().to_string(),
        "--json",
    ]);
    assert!(r.contains("\"members_added\":1.0"), "{r}");
    assert_eq!(
        ok(&["get", &db, "sales", "Jan", "Mitte", "sedan", "Sales"]).trim(),
        ""
    );
    std::fs::write(d.join("levels.csv"), "L1;L2\nTotal;North\nTotal;South\n").unwrap();
    let r = ok(&[
        "dim",
        "import",
        &db,
        "regions",
        &p(d, "levels.csv"),
        "--format",
        "levels",
        "--sync",
        "--delimiter",
        ";",
        "--json",
    ]);
    assert!(r.contains("\"members_removed\":3.0"), "{r}"); // East, West, Central
    assert!(
        fails(&["dim", "import", &db, "regions", &p(d, "missing.csv")]).contains("cannot read")
    );

    // journal records who did what
    let j = ok(&["journal", &db, "--by", "tester", "--limit", "100", "--json"]);
    for op in ["splash", "import", "rules", "dimension"] {
        assert!(
            j.contains(&format!("\"op\":\"{op}\"")),
            "{op} missing in {j}"
        );
    }

    // backup is an openable copy
    let bk = p(d, "backup.neolap");
    ok(&["backup", &db, &bk]);
    assert!(fails(&["backup", &db, &bk]).contains("already exists"));
    assert!(ok(&["info", &bk]).contains("sales"));
}

#[test]
fn model_export_and_import_roundtrip_through_git_style_files() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let (a, b) = (p(d, "a.neolap"), p(d, "b.neolap"));
    ok(&["demo", "tiny", &a]);
    let (m1, m2) = (p(d, "m1"), p(d, "m2"));
    ok(&["model", "export", &a, &m1]);
    for f in [
        "cubes/sales.toml",
        "dimensions/years.csv",
        "dimensions/measures.toml",
        "rules/sales.rules",
    ] {
        assert!(d.join("m1").join(f).exists(), "{f}");
    }
    ok(&["new", &b]);
    let r = ok(&["model", "import", &b, &m1, "--json"]);
    assert!(r.contains("years") && r.contains("sales"), "{r}");
    ok(&["model", "export", &b, &m2]);
    assert_eq!(read_tree(&d.join("m1")), read_tree(&d.join("m2")));
    // unchanged model → nothing to do
    let r = ok(&["model", "import", &b, &m1, "--json"]);
    assert!(
        r.contains("\"dimensions\":\"\"") && r.contains("\"cubes\":\"\""),
        "{r}"
    );
    // a damaged file is rejected with its place
    std::fs::write(d.join("m1/rules/sales.rules"), "['x'] = ;").unwrap();
    assert!(fails(&["model", "import", &b, &m1]).contains("line 1"));
    assert!(fails(&["model", "import", &b, &p(d, "nodir")]).contains("not found"));
}

fn read_tree(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            if e.path().is_dir() {
                stack.push(e.path());
            } else {
                out.push((
                    e.path().strip_prefix(dir).unwrap().to_path_buf(),
                    std::fs::read_to_string(e.path()).unwrap(),
                ));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn users_and_tokens_never_put_secrets_on_the_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let db = p(dir.path(), "u.neolap");
    ok(&["new", &db]);
    // the password comes from the environment, not from an argument
    let add = |name: &str, role: &str, pw: &str| {
        neolap()
            .args(["user", "add", &db, name, "--role", role])
            .env("NEOLAP_PASSWORD", pw)
            .output()
            .unwrap()
    };
    assert!(
        add("dirk", "admin", "a long enough secret")
            .status
            .success()
    );
    let short = add("anna", "planner", "tiny");
    assert!(
        !short.status.success() && String::from_utf8_lossy(&short.stderr).contains("at least 10")
    );
    assert!(
        !add("DIRK", "reader", "another long secret")
            .status
            .success()
    );
    assert!(!add("bob", "boss", "another long secret").status.success());
    assert!(
        add("anna", "planner", "another long secret")
            .status
            .success()
    );
    let list = ok(&["user", "list", &db, "--json"]);
    assert!(
        list.contains("\"dirk\"") && list.contains("\"admin\"") && list.contains("\"planner\"")
    );
    assert!(!list.contains("secret"));
    ok(&["user", "role", &db, "anna", "modeler"]);
    assert!(fails(&["user", "role", &db, "dirk", "reader"]).contains("last admin"));
    assert!(fails(&["user", "remove", &db, "dirk"]).contains("last admin"));
    // a token goes to stdout, the warning to stderr
    let o = neolap()
        .args(["user", "token", &db, "anna", "--label", "notebook"])
        .output()
        .unwrap();
    assert!(o.status.success());
    let token = String::from_utf8(o.stdout).unwrap();
    assert!(token.trim().starts_with("nlp_") && !token.trim().contains(' '));
    assert!(String::from_utf8_lossy(&o.stderr).contains("only once"));
    let pw = neolap()
        .args(["user", "password", &db, "anna"])
        .env("NEOLAP_PASSWORD", "a brand new secret")
        .output()
        .unwrap();
    assert!(pw.status.success());
    ok(&["user", "remove", &db, "anna"]);
    // the journal names the actor and holds no secret
    let j = ok(&["journal", &db, "--json"]);
    assert!(
        j.contains("user_add")
            && j.contains("token_create")
            && !j.contains("secret")
            && !j.contains(token.trim().rsplit('_').next().unwrap())
    );
}

#[test]
fn a_database_in_use_is_refused_and_errors_set_the_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let db = p(dir.path(), "x.neolap");
    ok(&["new", &db]);
    let held = neolap_core::Database::open(&db).unwrap();
    assert!(fails(&["info", &db]).contains("in use by another process"));
    drop(held);
    ok(&["info", &db]);
    assert!(fails(&["info", &p(dir.path(), "none.neolap")]).contains("not found"));
    std::fs::write(p(dir.path(), "junk.neolap"), "this is not sqlite").unwrap();
    assert!(!fails(&["info", &p(dir.path(), "junk.neolap")]).is_empty());
    let o = neolap().arg("version").output().unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).contains("NeOlap"));
    assert!(!neolap().output().unwrap().status.success()); // no command → usage error
}

/// Starts `neolap serve` on a free port with the given admin settings; returns the exit status if it
/// stopped by itself within a second, or `None` if it kept running (and was stopped by the test).
fn serve_with(
    user: Option<&str>,
    password: Option<&str>,
) -> (Option<std::process::ExitStatus>, String) {
    let dir = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut cmd = neolap();
    cmd.args([
        "serve",
        "--listen",
        &format!("127.0.0.1:{port}"),
        "--data-dir",
    ])
    .arg(dir.path());
    cmd.env_remove("NEOLAP_ADMIN_USER")
        .env_remove("NEOLAP_ADMIN_PASSWORD");
    if let Some(u) = user {
        cmd.env("NEOLAP_ADMIN_USER", u);
    }
    if let Some(p) = password {
        cmd.env("NEOLAP_ADMIN_PASSWORD", p);
    }
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Some(status) = child.try_wait().unwrap() {
            let mut err = String::new();
            std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut err).unwrap();
            return (Some(status), err);
        }
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
    }
    child.kill().ok();
    let out = child.wait_with_output().unwrap();
    (
        None,
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr),
    )
}

#[test]
fn the_server_starts_with_empty_or_complete_admin_settings_as_docker_compose_passes_them() {
    // Compose passes `${VAR:-}` as an empty string when nothing is set: that must still start
    let (exit, log) = serve_with(Some(""), Some(""));
    assert!(
        exit.is_none(),
        "must keep running, stopped with {exit:?}: {log}"
    );
    assert!(
        log.contains("no users"),
        "warns that nobody can log in: {log}"
    );
    let (exit, log) = serve_with(None, None);
    assert!(exit.is_none(), "{log}");
    let (exit, log) = serve_with(Some("dirk"), Some("a very long secret"));
    assert!(exit.is_none(), "{log}");
    assert!(
        log.contains("created the first administrator 'dirk'"),
        "{log}"
    );
    assert!(
        !log.contains("a very long secret"),
        "the password never reaches the log"
    );
}

#[test]
fn half_given_or_weak_admin_settings_stop_the_server_with_a_clear_message() {
    let (exit, err) = serve_with(Some("dirk"), None);
    assert!(
        exit.is_some_and(|s| !s.success()) && err.contains("NEOLAP_ADMIN_PASSWORD"),
        "{err}"
    );
    let (exit, err) = serve_with(Some(""), Some("a very long secret"));
    assert!(
        exit.is_some_and(|s| !s.success()) && err.contains("NEOLAP_ADMIN_USER"),
        "{err}"
    );
    let (exit, err) = serve_with(Some("dirk"), Some("short"));
    assert!(
        exit.is_some_and(|s| !s.success()) && err.contains("at least 10"),
        "{err}"
    );
}

/// Docker stops the server with SIGTERM and kills it after a grace period. An open live-update
/// stream (SSE) never ends by itself, so the server has to end it; otherwise a single open browser
/// tab keeps the server from stopping, and a running import would be cut off by the kill.
#[cfg(unix)]
#[test]
fn an_open_live_stream_does_not_keep_the_server_from_stopping() {
    use std::io::{Read, Write};
    let dir = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = neolap()
        .args([
            "serve",
            "--listen",
            &format!("127.0.0.1:{port}"),
            "--data-dir",
        ])
        .arg(dir.path())
        .env("NEOLAP_ADMIN_USER", "dirk")
        .env("NEOLAP_ADMIN_PASSWORD", "a very long secret")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let connect = || {
        for _ in 0..50 {
            if let Ok(s) = std::net::TcpStream::connect(("127.0.0.1", port)) {
                return s;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("the server did not start");
    };
    // log in over a raw socket, then open the event stream with the token
    let mut login = connect();
    let body = r#"{"user":"dirk","password":"a very long secret"}"#;
    write!(login, "POST /api/v1/login HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut resp = String::new();
    login.read_to_string(&mut resp).unwrap();
    let cookie = resp
        .lines()
        .find_map(|l| l.strip_prefix("set-cookie: "))
        .expect("a session cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let mut stream = connect();
    write!(stream, "GET /api/v1/events HTTP/1.1\r\nHost: x\r\nCookie: {cookie}\r\nAccept: text/event-stream\r\n\r\n").unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut head = [0u8; 256];
    let n = stream.read(&mut head).unwrap();
    assert!(
        String::from_utf8_lossy(&head[..n]).contains("text/event-stream"),
        "the stream is open"
    );
    // SIGTERM while the stream is still open
    let status = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let started = std::time::Instant::now();
    let exit = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the server is still running 5 s after SIGTERM with one open event stream"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(exit.success(), "a clean exit, not a crash: {exit:?}");
    drop(stream);
}
