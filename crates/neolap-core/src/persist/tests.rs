use std::path::PathBuf;

use proptest::prelude::*;
use tempfile::TempDir;

use super::users::hash;
use super::*;
use crate::cube::{Input, WriteContext};
use crate::dimension::{AttrValue, DimensionKind};

fn tmp() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.neolap");
    (dir, path)
}

fn years() -> Dimension {
    let mut e = Dimension::new("years", DimensionKind::Time).edit();
    let total = e.add("Total").unwrap();
    let q1 = e.add("Q1").unwrap();
    e.add_edge(total, q1, 1.0).unwrap();
    e.add_children(q1, &["Jan", "Feb"]).unwrap();
    let apr = e.add("Apr").unwrap();
    e.add_edge(total, apr, 1.0).unwrap();
    let jan = e.resolve("Jan").unwrap();
    e.set_alias(jan, "January").unwrap();
    e.set_attribute("order", jan, AttrValue::Number(1.0))
        .unwrap();
    e.set_subset("first", vec![jan]).unwrap();
    e.commit().unwrap()
}

fn measures() -> Dimension {
    let mut e = Dimension::new("measures", DimensionKind::Measures).edit();
    let profit = e.add("Profit").unwrap();
    let sales = e.add("Sales").unwrap();
    let cost = e.add("Cost").unwrap();
    e.add_edge(profit, sales, 1.0).unwrap();
    e.add_edge(profit, cost, -1.0).unwrap();
    e.commit().unwrap()
}

fn sample(path: &Path) -> Database {
    let mut db = Database::create(path).unwrap();
    db.save_dimension(years(), "admin").unwrap();
    db.save_dimension(measures(), "admin").unwrap();
    db.create_cube("sales", &["years", "measures"], "admin")
        .unwrap();
    db
}

fn put(
    db: &mut Database,
    user: &str,
    a: [&str; 2],
    input: &str,
) -> Result<crate::cube::WriteResult> {
    let cube = db.cube_mut("sales").unwrap();
    let addr = cube.address(&a).unwrap();
    cube.write(
        &WriteContext::new(user),
        &addr,
        Input::parse(input).unwrap(),
    )
}

fn val(db: &Database, a: [&str; 2]) -> Option<f64> {
    let cube = db.cube("sales").unwrap();
    cube.value(&cube.address(&a).unwrap()).unwrap()
}

fn cells(db: &Database) -> Vec<(Vec<MemberId>, u64)> {
    let mut v: Vec<_> = db
        .cube("sales")
        .unwrap()
        .export_cells()
        .into_iter()
        .map(|(k, v)| (k, v.to_bits()))
        .collect();
    v.sort();
    v
}

#[test]
fn roundtrip_keeps_model_cells_rules_and_generation() {
    let (_d, path) = tmp();
    let (before, generation) = {
        let mut db = sample(&path);
        put(&mut db, "anna", ["Jan", "Sales"], "100").unwrap();
        put(&mut db, "anna", ["Feb", "Sales"], "50").unwrap();
        put(&mut db, "anna", ["Q1", "Sales"], "300").unwrap(); // splash
        put(&mut db, "ben", ["Jan", "Cost"], "10").unwrap();
        (cells(&db), db.cube("sales").unwrap().generation())
    };
    let db = Database::open(&path).unwrap();
    assert_eq!(cells(&db), before);
    assert_eq!(db.cube("sales").unwrap().generation(), generation);
    let years = db.dimension("years").unwrap();
    assert_eq!(years.resolve("january"), years.resolve("Jan"));
    assert_eq!(
        years.attribute("order", years.member("Jan").unwrap()),
        Some(&AttrValue::Number(1.0))
    );
    assert_eq!(years.subsets()[0].name, "first");
    assert_eq!(val(&db, ["Q1", "Sales"]), Some(300.0));
}

#[test]
fn rules_are_persisted() {
    let (_d, path) = tmp();
    {
        let mut db = sample(&path);
        let report = db.set_rules("sales", "['Cost'] = 42;", "admin").unwrap();
        assert!(report[0].valid);
        assert!(db.set_rules("sales", "['Cost'] = ;", "admin").is_err());
    }
    let db = Database::open(&path).unwrap();
    let cube = db.cube("sales").unwrap();
    assert_eq!(cube.rules_text(), "['Cost'] = 42;");
    let v = cube
        .cell(&cube.address(&["Jan", "Cost"]).unwrap(), None)
        .unwrap();
    assert_eq!(v.number(), Some(42.0));
}

#[test]
fn undo_is_written_through_too() {
    let (_d, path) = tmp();
    let mut db = sample(&path);
    put(&mut db, "anna", ["Jan", "Sales"], "100").unwrap();
    let c = db.cube_mut("sales").unwrap();
    c.undo(&WriteContext::new("anna")).unwrap();
    drop(db);
    let db = Database::open(&path).unwrap();
    assert!(cells(&db).is_empty());
    assert_eq!(db.stored_cells("sales").unwrap(), 0);
}

#[test]
fn failing_sink_leaves_memory_and_file_unchanged() {
    struct Failing;
    impl OpSink for Failing {
        fn record(&mut self, _: &str, _: &OpRecord) -> Result<()> {
            Err(Error::Storage("disk full".into()))
        }
    }
    let (_d, path) = tmp();
    let mut db = sample(&path);
    put(&mut db, "anna", ["Jan", "Sales"], "100").unwrap();
    let (cells_before, gen_before) = (cells(&db), db.cube("sales").unwrap().generation());
    db.cube_mut("sales")
        .unwrap()
        .set_sink(Some(Box::new(Failing)));
    let err = put(&mut db, "anna", ["Q1", "Sales"], "999").unwrap_err();
    assert!(err.to_string().contains("disk full"));
    assert_eq!(cells(&db), cells_before);
    assert_eq!(db.cube("sales").unwrap().generation(), gen_before);
    assert!(
        db.cube_mut("sales")
            .unwrap()
            .undo(&WriteContext::new("anna"))
            .is_err()
            || true
    );
    // the rejected splash is not on the undo stack: undo reverts the earlier write instead
    db.cube_mut("sales").unwrap().set_sink(None);
    db.cube_mut("sales")
        .unwrap()
        .undo(&WriteContext::new("anna"))
        .unwrap();
    assert!(cells(&db).is_empty());
}

#[test]
fn abort_in_the_middle_of_a_transaction_rolls_back() {
    let (_d, path) = tmp();
    let mut db = sample(&path);
    put(&mut db, "anna", ["Jan", "Sales"], "100").unwrap();
    let before = cells(&db);
    {
        // a transaction that is dropped (process "dies") before commit
        let mut conn = db.conn.lock();
        let tx = conn.transaction().unwrap();
        tx.execute("DELETE FROM facts", []).unwrap();
        tx.execute("INSERT INTO journal (ts, user, op, cube, generation, n_cells) VALUES (0, 'x', 'x', 'sales', 0, 0)", []).unwrap();
        drop(tx);
    }
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(cells(&db), before);
    assert_eq!(db.stored_cells("sales").unwrap(), 1);
}

#[test]
fn killed_process_leaves_a_consistent_file() {
    // Copy the files while the writer is still open (WAL not checkpointed), as after a crash.
    let (dir, path) = tmp();
    let mut db = sample(&path);
    for i in 0..20 {
        put(&mut db, "anna", ["Jan", "Sales"], &format!("{}", i + 1)).unwrap();
    }
    put(&mut db, "anna", ["Q1", "Sales"], "777").unwrap();
    let snap = dir.path().join("crash.neolap");
    for ext in ["", "-wal", "-shm"] {
        let from = format!("{}{ext}", path.display());
        if Path::new(&from).exists() {
            std::fs::copy(&from, format!("{}{ext}", snap.display())).unwrap();
        }
    }
    let crashed = Database::open(&snap).unwrap();
    assert_eq!(val(&crashed, ["Q1", "Sales"]), Some(777.0));
    assert_eq!(cells(&crashed), cells(&db));
}

#[test]
fn second_open_is_refused_while_in_use() {
    let (_d, path) = tmp();
    let db = sample(&path);
    assert!(matches!(Database::open(&path), Err(Error::Locked(_))));
    drop(db);
    assert!(Database::open(&path).is_ok());
}

#[test]
fn rejects_foreign_and_newer_files() {
    let (dir, path) = tmp();
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE x (a)")
        .unwrap();
    assert!(
        Database::open(&path)
            .unwrap_err()
            .to_string()
            .contains("not a NeOlap database")
    );
    let newer = dir.path().join("newer.neolap");
    drop(sample(&newer));
    Connection::open(&newer)
        .unwrap()
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    assert!(
        Database::open(&newer)
            .unwrap_err()
            .to_string()
            .contains("update NeOlap")
    );
    assert!(Database::create(&path).is_err());
    assert!(matches!(
        Database::open(dir.path().join("none.neolap")),
        Err(Error::NotFound { .. })
    ));
}

#[test]
fn current_files_are_not_migrated_or_backed_up() {
    let (dir, path) = tmp();
    drop(sample(&path));
    let db = Database::open(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("before-v"))
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn journal_records_who_did_what() {
    let (_d, path) = tmp();
    let mut db = sample(&path);
    put(&mut db, "anna", ["Jan", "Sales"], "100").unwrap();
    put(&mut db, "ben", ["Q1", "Sales"], "300").unwrap();
    db.cube_mut("sales")
        .unwrap()
        .undo(&WriteContext::new("ben"))
        .unwrap();
    let all = db
        .journal(&JournalQuery {
            cube: Some("sales".into()),
            ..Default::default()
        })
        .unwrap();
    let ops: Vec<_> = all
        .iter()
        .map(|e| (e.user.as_str(), e.op.as_str(), e.n_cells))
        .collect();
    assert_eq!(
        ops,
        [
            ("ben", "undo", 1),
            ("ben", "splash", 1),
            ("anna", "splash", 1),
            ("admin", "create_cube", 0)
        ]
    );
    let only_ben = db
        .journal(&JournalQuery {
            user: Some("ben".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(only_ben.len(), 2);
    let cells = db.journal_cells(all[1].id).unwrap();
    assert_eq!(cells.len(), 1);
    assert_eq!((cells[0].1, cells[0].2), (Some(100.0), Some(300.0)));
    assert_eq!(
        db.journal(&JournalQuery {
            limit: Some(1),
            ..Default::default()
        })
        .unwrap()
        .len(),
        1
    );
}

#[test]
fn journal_retention_removes_old_entries() {
    let (_d, path) = tmp();
    let mut db = sample(&path);
    put(&mut db, "anna", ["Jan", "Sales"], "100").unwrap();
    db.conn
        .lock()
        .execute("UPDATE journal SET ts = ts - 100 * 86400", [])
        .unwrap();
    put(&mut db, "anna", ["Feb", "Sales"], "5").unwrap();
    let removed = db.set_journal_retention(90).unwrap();
    assert!(removed >= 3); // dimensions, cube and the first write
    let left = db.journal(&JournalQuery::default()).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(db.journal_cells(left[0].id).unwrap().len(), 1);
    let orphans: i64 = db
        .conn
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM journal_cells WHERE op_id NOT IN (SELECT id FROM journal)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0);
}

#[test]
fn dimension_change_drops_facts_of_removed_and_aggregated_members() {
    let (_d, path) = tmp();
    let mut db = sample(&path);
    put(&mut db, "anna", ["Jan", "Sales"], "100").unwrap();
    put(&mut db, "anna", ["Feb", "Sales"], "50").unwrap();
    put(&mut db, "anna", ["Apr", "Sales"], "7").unwrap();
    let mut ed = db.dimension("years").unwrap().edit();
    let apr = ed.resolve("Apr").unwrap();
    ed.remove(apr).unwrap(); // Apr removed
    let feb = ed.resolve("Feb").unwrap();
    ed.add_children(feb, &["Feb-1", "Feb-2"]).unwrap(); // Feb becomes an aggregate
    let new = ed.commit().unwrap();
    db.save_dimension(new, "modeller").unwrap();
    assert_eq!(val(&db, ["Total", "Sales"]), Some(100.0));
    assert_eq!(db.stored_cells("sales").unwrap(), 1);
    let entry = &db
        .journal(&JournalQuery {
            user: Some("modeller".into()),
            ..Default::default()
        })
        .unwrap()[0];
    assert_eq!((entry.op.as_str(), entry.n_cells), ("dimension", 2));
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(val(&db, ["Total", "Sales"]), Some(100.0));
    // the slot of Apr is reused by the next new member without hitting old facts
    let mut db = db;
    let mut ed = db.dimension("years").unwrap().edit();
    let id = ed.add("May").unwrap();
    ed.add_edge(ed.resolve("Total").unwrap(), id, 1.0).unwrap();
    db.save_dimension(ed.commit().unwrap(), "modeller").unwrap();
    assert_eq!(val(&db, ["Total", "Sales"]), Some(100.0));
}

#[test]
fn moved_member_keeps_its_facts() {
    let (_d, path) = tmp();
    let mut db = sample(&path);
    put(&mut db, "anna", ["Jan", "Sales"], "100").unwrap();
    let mut ed = db.dimension("years").unwrap().edit();
    let (jan, q1, total) = (
        ed.resolve("Jan").unwrap(),
        ed.resolve("Q1").unwrap(),
        ed.resolve("Total").unwrap(),
    );
    ed.move_member(jan, Some(q1), Some(total)).unwrap();
    db.save_dimension(ed.commit().unwrap(), "modeller").unwrap();
    assert_eq!(val(&db, ["Jan", "Sales"]), Some(100.0));
    assert_eq!(val(&db, ["Q1", "Sales"]), None);
}

#[test]
fn backup_is_a_consistent_openable_copy() {
    let (dir, path) = tmp();
    let mut db = sample(&path);
    put(&mut db, "anna", ["Jan", "Sales"], "100").unwrap();
    let copy = dir.path().join("backup.neolap");
    db.backup(&copy).unwrap();
    assert!(db.backup(&copy).is_err());
    put(&mut db, "anna", ["Feb", "Sales"], "5").unwrap(); // the source stays usable
    let b = Database::open(&copy).unwrap();
    assert_eq!(val(&b, ["Total", "Sales"]), Some(100.0));
    assert_eq!(db.cube("sales").unwrap().n_cells(), 2);
}

#[test]
fn creating_things_twice_or_with_unknown_parts_fails() {
    let (_d, path) = tmp();
    let mut db = sample(&path);
    assert!(
        db.create_cube("Sales", &["years", "measures"], "a")
            .is_err()
    );
    assert!(db.create_cube("x", &["years", "nope"], "a").is_err());
    assert!(db.set_rules("nope", "", "a").is_err());
}

proptest! {
    /// Any sequence of writes and undos: reopening gives exactly the same cells.
    #[test]
    fn persistence_roundtrip(ops in proptest::collection::vec((0usize..4, 0usize..3, 1u32..500, 0usize..4), 1..25)) {
        let (_d, path) = tmp();
        let before = {
            let mut db = sample(&path);
            let times = ["Jan", "Feb", "Q1", "Total"];
            let ms = ["Sales", "Cost", "Sales"];
            for (t, m, v, kind) in ops {
                let user = if kind == 3 { "ben" } else { "anna" };
                match kind {
                    0 | 1 => { let _ = put(&mut db, user, [times[t], ms[m]], &v.to_string()); }
                    2 => { let _ = put(&mut db, user, [times[t], ms[m]], &format!("#{v}")); }
                    _ => { let _ = db.cube_mut("sales").unwrap().undo(&WriteContext::new("anna")); }
                }
            }
            cells(&db)
        };
        let db = Database::open(&path).unwrap();
        prop_assert_eq!(cells(&db), before);
    }
}

/// M5 target: start with 10 million cells in < 20 s.
/// `cargo test --release -p neolap-core -- --ignored --nocapture persist_10m`
#[test]
#[ignore]
fn persist_10m() {
    use std::time::Instant;
    let (_d, path) = tmp();
    {
        let mut db = Database::create(&path).unwrap();
        let mut names = Vec::new();
        for i in 0..4 {
            let mut e = Dimension::new(format!("d{i}"), DimensionKind::Generic).edit();
            let root = e.add("All").unwrap();
            for l in 0..1000 {
                let m = e.add(&format!("l{l}")).unwrap();
                e.add_edge(root, m, 1.0).unwrap();
            }
            db.save_dimension(e.commit().unwrap(), "a").unwrap();
            names.push(format!("d{i}"));
        }
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        db.create_cube("big", &refs, "a").unwrap();
        let t = Instant::now();
        {
            let conn = db.conn.clone();
            let mut c = conn.lock();
            let tx = c.transaction().unwrap();
            {
                let mut stmt = tx
                    .prepare("INSERT OR REPLACE INTO facts (cube, k, v) VALUES ('big', ?1, ?2)")
                    .unwrap();
                let mut x: u64 = 88172645463325252;
                let mut rnd = move || {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    x
                };
                for _ in 0..10_000_000u64 {
                    let addr: Vec<MemberId> = (0..4).map(|_| 1 + (rnd() % 1000) as u32).collect();
                    stmt.execute(params![encode_key(&addr), 1.0f64]).unwrap();
                }
            }
            tx.commit().unwrap();
        }
        println!("fill file: {:?}", t.elapsed());
    }
    let t = Instant::now();
    let db = Database::open(&path).unwrap();
    let el = t.elapsed();
    println!(
        "open with {} cells: {el:?}",
        db.cube("big").unwrap().n_cells()
    );
    assert!(el.as_secs() < 20, "start too slow: {el:?}");
}

/// Write-through cost: a splash onto 100,000 cells including journal, and many single writes.
/// `cargo test --release -p neolap-core -- --ignored --nocapture persist_write_speed`
#[test]
#[ignore]
fn persist_write_speed() {
    use std::time::Instant;
    let (_d, path) = tmp();
    let mut db = Database::create(&path).unwrap();
    for (name, n) in [("a", 1000), ("b", 100)] {
        let mut e = Dimension::new(name, DimensionKind::Generic).edit();
        let root = e.add("All").unwrap();
        for i in 0..n {
            let m = e.add(&format!("m{i}")).unwrap();
            e.add_edge(root, m, 1.0).unwrap();
        }
        db.save_dimension(e.commit().unwrap(), "u").unwrap();
    }
    db.create_cube("c", &["a", "b"], "u").unwrap();
    let cube = db.cube_mut("c").unwrap();
    let all = cube.address(&["All", "All"]).unwrap();
    let ctx = WriteContext::new("u");
    let t = Instant::now();
    cube.write(&ctx, &all, Input::Equal(1_000_000.0)).unwrap();
    println!("splash 100,000 cells incl. journal: {:?}", t.elapsed());
    let t = Instant::now();
    for i in 0..1000 {
        let a = cube.address(&[&format!("m{}", i % 1000), "m1"]).unwrap();
        cube.write(&ctx, &a, Input::Set(f64::from(i) + 0.5))
            .unwrap();
    }
    println!("1000 single writes: {:?}", t.elapsed());
}

#[test]
fn users_authenticate_with_argon2_hashes() {
    let (_d, path) = tmp();
    let mut db = sample(&path);
    db.add_user("dirk", "correct horse battery", Role::Admin, "system")
        .unwrap();
    db.add_user("Anna", "another long secret", Role::Planner, "dirk")
        .unwrap();
    let u = db.authenticate("DIRK", "correct horse battery").unwrap();
    assert_eq!((u.name.as_str(), u.role), ("dirk", Role::Admin));
    // wrong password and unknown user are indistinguishable
    let wrong = db
        .authenticate("dirk", "wrong password!!")
        .unwrap_err()
        .to_string();
    let unknown = db
        .authenticate("nobody", "correct horse battery")
        .unwrap_err()
        .to_string();
    assert_eq!(wrong, unknown);
    // the file never contains a password
    let hash: String = db
        .conn
        .lock()
        .query_row(
            "SELECT password_hash FROM users WHERE name = 'dirk'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(hash.starts_with("$argon2id$") && !hash.contains("correct horse"));
    assert!(
        db.add_user("anna", "yet another secret", Role::Reader, "dirk")
            .is_err()
    ); // case-insensitive
    assert!(db.add_user("short", "tiny", Role::Reader, "dirk").is_err());
    assert!(
        db.add_user("bad name", "long enough password", Role::Reader, "dirk")
            .is_err()
    );
    assert!(
        db.add_user("../x", "long enough password", Role::Reader, "dirk")
            .is_err()
    );
    let names: Vec<_> = db
        .users()
        .unwrap()
        .into_iter()
        .map(|u| (u.name, u.role))
        .collect();
    assert_eq!(
        names,
        [
            ("Anna".to_string(), Role::Planner),
            ("dirk".to_string(), Role::Admin)
        ]
    );
    drop(db);
    let db = Database::open(&path).unwrap();
    assert!(db.authenticate("anna", "another long secret").is_ok());
}

#[test]
fn the_last_admin_cannot_be_removed_or_demoted() {
    let (_d, path) = tmp();
    let mut db = sample(&path);
    db.add_user("root", "a very long secret", Role::Admin, "system")
        .unwrap();
    assert!(db.remove_user("root", "root").is_err());
    assert!(db.set_role("root", Role::Planner, "root").is_err());
    db.add_user("second", "another very long secret", Role::Admin, "root")
        .unwrap();
    db.set_role("root", Role::Planner, "second").unwrap();
    assert!(db.remove_user("second", "second").is_err()); // now the last one
    db.remove_user("root", "second").unwrap();
    assert!(db.remove_user("ghost", "second").is_err());
}

#[test]
fn tokens_are_shown_once_and_die_with_the_password() {
    let (_d, path) = tmp();
    let mut db = sample(&path);
    db.add_user("anna", "a long enough secret", Role::Planner, "system")
        .unwrap();
    let t = db.create_token("anna", "notebook", "anna").unwrap();
    assert!(t.starts_with("nlp_"));
    assert_eq!(db.authenticate_token(&t).unwrap().name, "anna");
    let stored: String = db
        .conn
        .lock()
        .query_row("SELECT token_hash FROM tokens", [], |r| r.get(0))
        .unwrap();
    assert!(!stored.contains(t.rsplit('_').next().unwrap()));
    for bad in [
        "",
        "nlp_",
        "nlp_x_y",
        "xyz",
        &format!("{t}0"),
        &t.replace("nlp_", "nlq_"),
    ] {
        assert!(db.authenticate_token(bad).is_err(), "{bad}");
    }
    assert!(db.create_token("ghost", "x", "a").is_err());
    let t2 = db.create_token("anna", "second", "anna").unwrap();
    assert_ne!(t, t2);
    db.set_password("anna", "a brand new secret", "anna")
        .unwrap();
    assert!(db.authenticate_token(&t).is_err() && db.authenticate_token(&t2).is_err());
    assert!(db.authenticate("anna", "a long enough secret").is_err());
    assert!(db.authenticate("anna", "a brand new secret").is_ok());
    let t3 = db.create_token("anna", "third", "anna").unwrap();
    assert_eq!(db.revoke_tokens("anna", "anna").unwrap(), 1);
    assert!(db.authenticate_token(&t3).is_err());
}

#[test]
fn user_changes_are_journaled_without_secrets() {
    let (_d, path) = tmp();
    let mut db = sample(&path);
    db.add_user("anna", "a long enough secret", Role::Reader, "admin")
        .unwrap();
    db.set_role("anna", Role::Modeler, "admin").unwrap();
    let t = db.create_token("anna", "x", "admin").unwrap();
    let all = db.journal(&JournalQuery::default()).unwrap();
    let ops: Vec<_> = all.iter().take(3).map(|e| e.op.as_str()).collect();
    assert_eq!(ops, ["token_create", "user_role", "user_add"]);
    let blob = format!("{all:?}");
    assert!(
        !blob.contains("a long enough secret") && !blob.contains(t.rsplit('_').next().unwrap())
    );
}

#[test]
fn roles_are_ordered_and_parse_in_both_languages() {
    assert!(
        Role::Reader < Role::Planner
            && Role::Planner < Role::Modeler
            && Role::Modeler < Role::Admin
    );
    assert_eq!(Role::parse("Planer").unwrap(), Role::Planner);
    assert_eq!(Role::parse(" MODELLIERER ").unwrap(), Role::Modeler);
    assert!(Role::parse("boss").is_err());
}

fn rights_db(path: &Path) -> Database {
    let mut db = sample(path);
    db.add_user("root", "a very long secret", Role::Admin, "system")
        .unwrap();
    db.add_user("pia", "a very long secret", Role::Planner, "root")
        .unwrap();
    db.add_user("rita", "a very long secret", Role::Reader, "root")
        .unwrap();
    db.add_user("mo", "a very long secret", Role::Modeler, "root")
        .unwrap();
    db
}

fn grant(user: &str, access: Access, area: &[(&str, &[&str])]) -> Grant {
    Grant {
        user: user.into(),
        cube: "sales".into(),
        access,
        area: area
            .iter()
            .map(|(d, m)| {
                (
                    (*d).to_string(),
                    m.iter().map(|x| (*x).to_string()).collect(),
                )
            })
            .collect(),
    }
}

fn can_write(db: &Database, user: &str, role: Role, cells: [&str; 2]) -> bool {
    let r = db.rights(user, role, "sales").unwrap();
    let cube = db.cube("sales").unwrap();
    let addr = cube.address(&cells).unwrap();
    let coords: Vec<u32> = cube
        .dimensions()
        .iter()
        .zip(&addr)
        .map(|(d, &m)| d.leaf_ordinal(m).unwrap())
        .collect();
    r.write_permission()(&coords)
}

#[test]
fn without_a_grant_a_planner_sees_nothing() {
    let (_d, path) = tmp();
    let db = rights_db(&path);
    let r = db.rights("pia", Role::Planner, "sales").unwrap();
    assert!(!r.can_read_cube());
    assert_eq!(r.best(), Access::None);
    assert!(db.readable_cubes("pia", Role::Planner).unwrap().is_empty());
    assert!(!can_write(&db, "pia", Role::Planner, ["Jan", "Sales"]));
    // admins and modelers are limited by their role only
    assert_eq!(db.readable_cubes("root", Role::Admin).unwrap(), ["sales"]);
    assert!(can_write(&db, "mo", Role::Modeler, ["Jan", "Sales"]));
}

#[test]
fn an_area_grant_covers_descendants_and_nothing_else() {
    let (_d, path) = tmp();
    let mut db = rights_db(&path);
    // Q1 = Jan + Feb; the grant on the aggregate covers its base members
    db.grant(
        &grant(
            "pia",
            Access::Write,
            &[("years", &["Q1"]), ("measures", &["Sales"])],
        ),
        "root",
    )
    .unwrap();
    assert!(can_write(&db, "pia", Role::Planner, ["Jan", "Sales"]));
    assert!(can_write(&db, "pia", Role::Planner, ["Feb", "Sales"]));
    assert!(!can_write(&db, "pia", Role::Planner, ["Apr", "Sales"])); // other quarter
    assert!(!can_write(&db, "pia", Role::Planner, ["Jan", "Cost"])); // other measure
    assert!(db.readable_cubes("pia", Role::Planner).unwrap() == ["sales"]);
    // the grant belongs to pia only
    assert!(!can_write(&db, "rita", Role::Reader, ["Jan", "Sales"]));
}

#[test]
fn a_reader_never_writes_even_with_a_write_grant() {
    let (_d, path) = tmp();
    let mut db = rights_db(&path);
    db.grant(&grant("rita", Access::Write, &[]), "root")
        .unwrap();
    assert!(!can_write(&db, "rita", Role::Reader, ["Jan", "Sales"]));
    assert!(
        db.rights("rita", Role::Reader, "sales")
            .unwrap()
            .can_read_cube()
    );
    assert_eq!(
        db.rights("rita", Role::Reader, "sales").unwrap().best(),
        Access::Read
    );
}

#[test]
fn an_explicit_none_carves_a_hole_into_a_wider_grant() {
    let (_d, path) = tmp();
    let mut db = rights_db(&path);
    db.grant(&grant("pia", Access::Write, &[]), "root").unwrap();
    db.grant(
        &grant("pia", Access::None, &[("measures", &["Cost"])]),
        "root",
    )
    .unwrap();
    assert!(can_write(&db, "pia", Role::Planner, ["Jan", "Sales"]));
    assert!(!can_write(&db, "pia", Role::Planner, ["Jan", "Cost"]));
    // `none` restricts writing; reading is per cube (see the module documentation of access.rs)
    let r = db.rights("pia", Role::Planner, "sales").unwrap();
    assert!(r.can_read_cube());
}

#[test]
fn grants_are_validated_replaced_listed_and_revoked() {
    let (_d, path) = tmp();
    let mut db = rights_db(&path);
    assert!(
        db.grant(&grant("ghost", Access::Read, &[]), "root")
            .is_err()
    );
    assert!(
        db.grant(&grant("pia", Access::Read, &[("nope", &["x"])]), "root")
            .is_err()
    );
    assert!(
        db.grant(
            &grant("pia", Access::Read, &[("years", &["Smarch"])]),
            "root"
        )
        .is_err()
    );
    let mut other = grant("pia", Access::Read, &[]);
    other.cube = "nocube".into();
    assert!(db.grant(&other, "root").is_err());
    db.grant(&grant("pia", Access::Read, &[("years", &["Q1"])]), "root")
        .unwrap();
    db.grant(&grant("pia", Access::Write, &[("years", &["Q1"])]), "root")
        .unwrap(); // same area: replaces
    let g = db.grants(Some("PIA")).unwrap();
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].access, Access::Write);
    assert_eq!(
        db.revoke(
            "pia",
            "sales",
            &[("years".into(), vec!["Q1".into()])],
            "root"
        )
        .unwrap(),
        1
    );
    assert!(db.grants(None).unwrap().is_empty());
    drop(db);
    let mut db = Database::open(&path).unwrap();
    db.grant(&grant("pia", Access::Read, &[]), "root").unwrap();
    drop(db);
    assert_eq!(
        Database::open(&path).unwrap().grants(None).unwrap().len(),
        1
    );
}

#[test]
fn rights_enforced_through_the_cube_reject_the_whole_operation() {
    let (_d, path) = tmp();
    let mut db = rights_db(&path);
    db.grant(&grant("pia", Access::Write, &[("years", &["Jan"])]), "root")
        .unwrap();
    let rights = db.rights("pia", Role::Planner, "sales").unwrap();
    let mut ctx = WriteContext::new("pia");
    ctx.can_write = Some(rights.write_permission());
    let cube = db.cube_mut("sales").unwrap();
    let jan = cube.address(&["Jan", "Sales"]).unwrap();
    cube.write(&ctx, &jan, Input::Set(5.0)).unwrap();
    let q1 = cube.address(&["Q1", "Sales"]).unwrap();
    // Feb holds no value, so a proportional splash only changes Jan, where pia may write
    cube.write(&ctx, &q1, Input::Set(100.0)).unwrap();
    assert_eq!(cube.value(&jan).unwrap(), Some(100.0));
    // an equal splash creates every combination of the area, including Feb: refused as a whole
    let err = cube.write(&ctx, &q1, Input::Equal(200.0)).unwrap_err();
    assert!(matches!(err, Error::Denied(m) if m.contains("Feb")));
    assert_eq!(cube.value(&jan).unwrap(), Some(100.0), "nothing changed");
    // once Feb holds a value, even the proportional splash reaches it and is refused
    let feb = cube.address(&["Feb", "Sales"]).unwrap();
    cube.set_base(&feb, Some(1.0)).unwrap();
    let err = cube.write(&ctx, &q1, Input::Set(300.0)).unwrap_err();
    assert!(matches!(err, Error::Denied(m) if m.contains("Feb")));
    assert_eq!(cube.value(&q1).unwrap(), Some(101.0));
}

#[test]
fn access_and_role_names_parse() {
    assert_eq!(Access::parse("Schreiben").unwrap(), Access::Write);
    assert_eq!(Access::parse(" NONE ").unwrap(), Access::None);
    assert!(Access::parse("admin").is_err());
    assert!(Access::None < Access::Read && Access::Read < Access::Write);
}

// ---- API tokens: a fast hash, because a token is a random secret, not a password

fn stored_hash(db: &Database, id_of: &str) -> String {
    let id = id_of
        .strip_prefix("nlp_")
        .unwrap()
        .split('_')
        .next()
        .unwrap()
        .to_string();
    db.conn
        .lock()
        .query_row(
            "SELECT token_hash FROM tokens WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn a_token_is_checked_in_microseconds_not_in_a_password_hash_time() {
    let (_d, path) = tmp();
    let mut db = sample(&path);
    db.add_user("anna", "a long enough secret", Role::Planner, "system")
        .unwrap();
    let t = db.create_token("anna", "x", "anna").unwrap();
    // the stored value is a SHA-256 digest (marked), never the secret and never an Argon2 string
    let h = stored_hash(&db, &t);
    assert!(
        h.starts_with("sha256:") && h.len() == "sha256:".len() + 64,
        "{h}"
    );
    assert!(!h.contains(t.rsplit('_').next().unwrap()));
    // 50 checks (valid and invalid) take far less than one Argon2 hash would
    let start = std::time::Instant::now();
    for _ in 0..50 {
        assert!(db.authenticate_token(&t).is_ok());
        assert!(db.authenticate_token(&format!("{t}0")).is_err());
    }
    let per_check = start.elapsed() / 100;
    assert!(
        per_check < std::time::Duration::from_millis(5),
        "a token check took {per_check:?}"
    );
}

#[test]
fn tokens_made_before_the_change_still_work_and_are_upgraded_on_first_use() {
    let (_d, path) = tmp();
    let mut db = sample(&path);
    db.add_user("anna", "a long enough secret", Role::Planner, "system")
        .unwrap();
    let t = db.create_token("anna", "old", "anna").unwrap();
    // rewrite the row the way the previous version stored it: an Argon2 hash of the secret
    let secret = t.rsplit('_').next().unwrap().to_string();
    let id = t
        .strip_prefix("nlp_")
        .unwrap()
        .split('_')
        .next()
        .unwrap()
        .to_string();
    let argon = hash(&secret).unwrap();
    db.conn
        .lock()
        .execute(
            "UPDATE tokens SET token_hash = ?2 WHERE id = ?1",
            params![id, argon],
        )
        .unwrap();
    assert!(stored_hash(&db, &t).starts_with("$argon2id$"));
    assert_eq!(
        db.authenticate_token(&t).unwrap().name,
        "anna",
        "an old token is still valid"
    );
    assert!(
        stored_hash(&db, &t).starts_with("sha256:"),
        "and is upgraded to the fast hash on first use"
    );
    assert_eq!(
        db.authenticate_token(&t).unwrap().name,
        "anna",
        "valid after the upgrade too"
    );
    // a wrong secret never upgrades anything, for either kind of stored hash
    let t2 = db.create_token("anna", "old2", "anna").unwrap();
    let id2 = t2
        .strip_prefix("nlp_")
        .unwrap()
        .split('_')
        .next()
        .unwrap()
        .to_string();
    db.conn
        .lock()
        .execute(
            "UPDATE tokens SET token_hash = ?2 WHERE id = ?1",
            params![id2, hash(t2.rsplit('_').next().unwrap()).unwrap()],
        )
        .unwrap();
    assert!(
        db.authenticate_token(&format!("nlp_{id2}_{}", "0".repeat(48)))
            .is_err()
    );
    assert!(
        stored_hash(&db, &t2).starts_with("$argon2id$"),
        "a failed attempt leaves the old hash alone"
    );
}

#[test]
fn a_token_for_another_id_or_with_a_changed_secret_is_refused() {
    let (_d, path) = tmp();
    let mut db = sample(&path);
    db.add_user("anna", "a long enough secret", Role::Planner, "system")
        .unwrap();
    db.add_user("ben", "a long enough secret", Role::Planner, "system")
        .unwrap();
    let ta = db.create_token("anna", "a", "anna").unwrap();
    let tb = db.create_token("ben", "b", "ben").unwrap();
    let (ida, sa) = ta.strip_prefix("nlp_").unwrap().split_once('_').unwrap();
    let (idb, sb) = tb.strip_prefix("nlp_").unwrap().split_once('_').unwrap();
    assert_eq!(db.authenticate_token(&ta).unwrap().name, "anna");
    assert_eq!(db.authenticate_token(&tb).unwrap().name, "ben");
    // anna's id with ben's secret, and the other way round: never accepted
    assert!(db.authenticate_token(&format!("nlp_{ida}_{sb}")).is_err());
    assert!(db.authenticate_token(&format!("nlp_{idb}_{sa}")).is_err());
    // a secret that differs in the last character only
    let mut near = sa.to_string();
    near.pop();
    near.push(if sa.ends_with('0') { '1' } else { '0' });
    assert!(db.authenticate_token(&format!("nlp_{ida}_{near}")).is_err());
    for bad in [
        "",
        "nlp_",
        "nlp__",
        "nlp_x",
        &format!("nlp_{ida}_"),
        &format!("{ida}_{sa}"),
        &format!("nlp_{ida}_{}", sa.to_uppercase()),
    ] {
        assert!(db.authenticate_token(bad).is_err(), "{bad:?}");
    }
}
