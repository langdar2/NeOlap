use std::sync::Arc;

use proptest::prelude::*;

use super::*;
use crate::cube::{Cube, CubeProvider, Input, WriteContext};
use crate::dimension::{Dimension, DimensionKind};
use crate::persist::Database;
use crate::{Error, MemberId};

fn dims() -> (Dimension, Dimension, Dimension) {
    let mut y = Dimension::new("years", DimensionKind::Time).edit();
    let total = y.add("Year").unwrap();
    for (q, months) in [("Q1", ["Jan", "Feb"]), ("Q2", ["Apr", "May"])] {
        let q = y.add(q).unwrap();
        y.add_edge(total, q, 1.0).unwrap();
        y.add_children(q, &months).unwrap();
    }
    let jan = y.resolve("Jan").unwrap();
    y.set_alias(jan, "Januar").unwrap();
    let mut r = Dimension::new("regions", DimensionKind::Generic).edit();
    let all = r.add("Total").unwrap();
    r.add_children(all, &["Nord", "Sued"]).unwrap();
    let mut m = Dimension::new("measures", DimensionKind::Measures).edit();
    let profit = m.add("Profit").unwrap();
    m.add_children(profit, &["Sales"]).unwrap();
    let cost = m.add("Cost").unwrap();
    m.add_edge(profit, cost, -1.0).unwrap();
    (
        y.commit().unwrap(),
        r.commit().unwrap(),
        m.commit().unwrap(),
    )
}

fn cube() -> Cube {
    let (y, r, m) = dims();
    Cube::new("sales", vec![Arc::new(y), Arc::new(r), Arc::new(m)])
}

const WIDE: &str = r#"
name = "Ist-Umsatz laden"
cube = "sales"
source = { file = "ist.csv", delimiter = ";", decimal = "," }

[map]
years   = { column = "Monat", alias = true }
regions = "Region"

[values]
Sales = "Umsatz"
Cost  = "Kosten"

[options]
mode = "replace_area"
unknown_member = "reject"
"#;

const FILE: &str =
    "Monat;Region;Umsatz;Kosten\nJanuar;Nord;1.200,50;300\nFeb;Nord;100;\nJan;Sued;50;25,5\n";

fn spec(text: &str) -> ImportSpec {
    ImportSpec::from_toml(text).unwrap()
}

fn ctx() -> WriteContext {
    WriteContext::new("anna")
}

fn run(c: &mut Cube, s: &ImportSpec, data: &str, dry: bool) -> ImportReport {
    run_import(c, s, SourceKind::Csv, data.as_bytes(), &ctx(), dry, None).unwrap()
}

fn v(c: &Cube, a: [&str; 3]) -> Option<f64> {
    c.value(&c.address(&a).unwrap()).unwrap()
}

fn snapshot(c: &Cube) -> Vec<(Vec<MemberId>, u64)> {
    let mut s: Vec<_> = c
        .export_cells()
        .into_iter()
        .map(|(k, x)| (k, x.to_bits()))
        .collect();
    s.sort();
    s
}

#[test]
fn wide_import_from_the_plan() {
    let mut c = cube();
    let r = run(&mut c, &spec(WIDE), FILE, false);
    assert_eq!(
        (r.rows_read, r.cells_read, r.cells_created, r.rows_rejected),
        (3, 5, 5, 0)
    );
    assert!(r.failed.is_none() && r.op_id.is_some());
    assert_eq!(v(&c, ["Jan", "Nord", "Sales"]), Some(1200.5)); // alias + decimal comma + thousands dot
    assert_eq!(v(&c, ["Jan", "Nord", "Cost"]), Some(300.0));
    assert_eq!(v(&c, ["Feb", "Nord", "Cost"]), None); // empty value cell = no cell
    assert_eq!(v(&c, ["Q1", "Total", "Sales"]), Some(1350.5));
    assert_eq!(v(&c, ["Q1", "Total", "Profit"]), Some(1350.5 - 325.5));
}

#[test]
fn replace_area_twice_equals_once() {
    let mut once = cube();
    run(&mut once, &spec(WIDE), FILE, false);
    let mut twice = cube();
    run(&mut twice, &spec(WIDE), FILE, false);
    let second = run(&mut twice, &spec(WIDE), FILE, false);
    assert_eq!(snapshot(&once), snapshot(&twice));
    assert_eq!(
        (
            second.cells_created,
            second.cells_updated,
            second.cells_deleted
        ),
        (0, 0, 0)
    );
    assert_eq!(second.cells_unchanged, 5);
    assert_eq!(second.op_id, None);
}

#[test]
fn replace_area_clears_exactly_the_covered_area() {
    let mut c = cube();
    run(&mut c, &spec(WIDE), FILE, false);
    // Apr/Nord is outside the area of the next file (only Jan, Nord / Sales); Feb/Nord inside the
    // covered members → replaced, so it disappears
    let a = c.address(&["Apr", "Nord", "Sales"]).unwrap();
    c.write(&ctx(), &a, Input::Set(7.0)).unwrap();
    let r = run(
        &mut c,
        &spec(WIDE),
        "Monat;Region;Umsatz;Kosten\nJan;Nord;1;\nFeb;Sued;2;\n",
        false,
    );
    assert_eq!(v(&c, ["Apr", "Nord", "Sales"]), Some(7.0)); // Apr is not covered
    assert_eq!(v(&c, ["Jan", "Nord", "Sales"]), Some(1.0));
    // Cost never occurs with a value in the file, so it is not part of the covered area
    assert_eq!(v(&c, ["Jan", "Nord", "Cost"]), Some(300.0));
    assert_eq!(v(&c, ["Jan", "Sued", "Sales"]), None); // Jan × Sued × Sales covered → removed
    assert_eq!(r.cells_deleted, 2); // Feb/Nord/Sales and Jan/Sued/Sales; Feb/Nord had no Cost cell
}

#[test]
fn add_and_overwrite_modes() {
    let mut c = cube();
    let add = WIDE.replace("replace_area", "add");
    run(&mut c, &spec(&add), FILE, false);
    run(&mut c, &spec(&add), FILE, false);
    assert_eq!(v(&c, ["Jan", "Nord", "Sales"]), Some(2401.0));
    let over = WIDE.replace("replace_area", "overwrite");
    run(&mut c, &spec(&over), FILE, false);
    assert_eq!(v(&c, ["Jan", "Nord", "Sales"]), Some(1200.5));
    assert_eq!(v(&c, ["Feb", "Nord", "Cost"]), None);
}

#[test]
fn long_format_with_fixed_member_and_summed_duplicates() {
    let s = spec(
        r#"
name = "lang"
cube = "sales"
value_column = "Wert"
[map]
years = "Monat"
regions = "Region"
measures = "Kennzahl"
[fixed]
[options]
mode = "add"
"#,
    );
    let mut c = cube();
    let r = run(
        &mut c,
        &s,
        "Monat,Region,Kennzahl,Wert\nJan,Nord,Sales,10\nJan,Nord,Sales,5\nJan,Nord,Cost,-2\n",
        false,
    );
    assert_eq!(r.cells_read, 3);
    assert_eq!(v(&c, ["Jan", "Nord", "Sales"]), Some(15.0));
    let fixed = spec(
        r#"
name = "fix"
cube = "sales"
value_column = "Wert"
[map]
years = "Monat"
measures = "Kennzahl"
[fixed]
regions = "Nord"
"#,
    );
    run(&mut c, &fixed, "Monat,Kennzahl,Wert\nFeb,Sales,3\n", false);
    assert_eq!(v(&c, ["Feb", "Nord", "Sales"]), Some(3.0));
}

#[test]
fn reject_stops_everything_and_reports_rows() {
    let mut c = cube();
    let before = c.generation();
    let r = run(
        &mut c,
        &spec(WIDE),
        "Monat;Region;Umsatz;Kosten\nJan;Nord;1;1\nXyz;Nord;1;1\nJan;Mitte;1;1\nFeb;Nord;abc;1\nQ1;Nord;1;1\n",
        false,
    );
    assert!(r.failed.is_some());
    assert_eq!((r.rows_read, r.rows_rejected), (5, 4));
    let reasons: Vec<_> = r
        .rejected
        .iter()
        .map(|x| (x.line, x.reason.as_str()))
        .collect();
    assert_eq!(reasons[0].0, 3);
    assert!(reasons[0].1.contains("unknown member 'Xyz'"));
    assert!(reasons[1].1.contains("Mitte"));
    assert!(reasons[2].1.contains("'abc'") && reasons[2].1.contains("Umsatz"));
    assert!(reasons[3].1.contains("aggregate"));
    assert_eq!((c.generation(), c.n_cells()), (before, 0));
}

#[test]
fn skip_policy_loads_the_good_rows() {
    let mut c = cube();
    let s = spec(&WIDE.replace("reject", "skip"));
    let r = run(
        &mut c,
        &s,
        "Monat;Region;Umsatz;Kosten\nJan;Nord;1;1\nXyz;Nord;1;1\nFeb;Sued;5;\n",
        false,
    );
    assert!(r.failed.is_none());
    assert_eq!((r.rows_rejected, r.cells_created), (1, 3));
    assert_eq!(v(&c, ["Feb", "Sued", "Sales"]), Some(5.0));
}

#[test]
fn dry_run_changes_nothing_but_reports_the_same() {
    let mut c = cube();
    run(&mut c, &spec(WIDE), FILE, false);
    let (snap, generation) = (snapshot(&c), c.generation());
    let changed = "Monat;Region;Umsatz;Kosten\nJan;Nord;9;\n";
    let dry = run(&mut c, &spec(WIDE), changed, true);
    assert!(dry.dry_run && dry.op_id.is_none());
    assert_eq!((snapshot(&c), c.generation()), (snap, generation));
    let real = run(&mut c, &spec(WIDE), changed, false);
    assert_eq!(
        (dry.cells_created, dry.cells_updated, dry.cells_deleted),
        (real.cells_created, real.cells_updated, real.cells_deleted)
    );
}

#[test]
fn an_import_is_one_undo_step() {
    let mut c = cube();
    run(&mut c, &spec(WIDE), FILE, false);
    let before = snapshot(&c);
    let s = spec(&WIDE.replace("replace_area", "overwrite"));
    run(
        &mut c,
        &s,
        "Monat;Region;Umsatz;Kosten\nJan;Nord;9;1\nApr;Sued;4;\n",
        false,
    );
    assert_ne!(snapshot(&c), before);
    c.undo(&ctx()).unwrap();
    assert_eq!(snapshot(&c), before);
}

#[test]
fn rules_and_permissions_protect_the_import() {
    let mut c = cube();
    c.set_rules("['Cost'] = 1;").unwrap();
    let r = run_import(
        &mut c,
        &spec(WIDE),
        SourceKind::Csv,
        FILE.as_bytes(),
        &ctx(),
        false,
        None,
    );
    assert!(matches!(r, Err(Error::Denied(m)) if m.contains("rule in line 1")));
    let mut c = cube();
    let mut limited = ctx();
    limited.can_write = Some(Arc::new(|coords| coords[1] == 0)); // only the first region
    let r = run_import(
        &mut c,
        &spec(WIDE),
        SourceKind::Csv,
        FILE.as_bytes(),
        &limited,
        false,
        None,
    );
    assert!(matches!(r, Err(Error::Denied(m)) if m.contains("Sued")));
    assert_eq!(c.n_cells(), 0); // all or nothing
}

#[test]
fn conflicting_changes_are_detected() {
    let mut c = cube();
    let seen = c.generation();
    let a = c.address(&["Jan", "Nord", "Sales"]).unwrap();
    c.write(&WriteContext::new("ben"), &a, Input::Set(5.0))
        .unwrap();
    let r = run_import(
        &mut c,
        &spec(WIDE),
        SourceKind::Csv,
        FILE.as_bytes(),
        &ctx().expecting(seen),
        false,
        None,
    );
    assert!(matches!(r, Err(Error::WriteConflict { user, .. }) if user == "ben"));
}

#[test]
fn mapping_errors_are_clear() {
    let mut c = cube();
    let try_spec = |c: &mut Cube, toml: &str, data: &str| {
        run_import(
            c,
            &spec(toml),
            SourceKind::Csv,
            data.as_bytes(),
            &ctx(),
            false,
            None,
        )
        .unwrap_err()
        .to_string()
    };
    let missing = WIDE.replace("regions = \"Region\"", "");
    assert!(try_spec(&mut c, &missing, FILE).contains("regions not mapped"));
    let col = WIDE.replace("\"Region\"", "\"Gebiet\"");
    let e = try_spec(&mut c, &col, FILE);
    assert!(e.contains("column 'Gebiet' not found") && e.contains("Monat"));
    let twice = WIDE.replace("[map]", "[map]\nyears = \"Region\"");
    assert!(ImportSpec::from_toml(&twice).is_err());
    assert!(ImportSpec::from_toml("name = 'x'\ncube = 'c'\nbogus = 1").is_err());
    assert!(ImportSpec::from_toml(&WIDE.replace("reject", "create_under:")).is_err());
    assert!(ImportSpec::from_toml(&WIDE.replace("reject", "maybe")).is_err());
}

#[test]
fn xlsx_roundtrip_via_export() {
    let mut c = cube();
    run(&mut c, &spec(WIDE), FILE, false);
    let t = export_table(
        &c,
        &ExportSpec {
            area: vec![("measures".into(), vec!["Sales".into(), "Cost".into()])],
            ..Default::default()
        },
        None,
    )
    .unwrap();
    let bytes = to_xlsx(&t, "Umsatz [2026]").unwrap();
    let s = spec(
        r#"
name = "zurueck"
cube = "sales"
value_column = "value"
[map]
years = "years"
regions = "regions"
measures = "measures"
[options]
mode = "replace_area"
"#,
    );
    let mut d = cube();
    let r = run_import(&mut d, &s, SourceKind::Xlsx, &bytes, &ctx(), false, None).unwrap();
    assert!(r.failed.is_none(), "{r:?}");
    assert_eq!(snapshot(&c), snapshot(&d));
}

#[test]
fn export_layouts_aggregates_and_csv() {
    let mut c = cube();
    run(&mut c, &spec(WIDE), FILE, false);
    let long = export_table(&c, &ExportSpec::default(), None).unwrap();
    assert_eq!(long.headers, ["years", "regions", "measures", "value"]);
    assert_eq!(long.rows.len(), 5);
    let wide = export_table(
        &c,
        &ExportSpec {
            layout: Layout::Wide {
                dimension: "measures".into(),
            },
            aggregates: true,
            area: vec![
                ("years".into(), vec!["Q1".into()]),
                ("regions".into(), vec!["Total".into()]),
            ],
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(
        wide.headers,
        ["years", "regions", "Sales", "Cost", "Profit"]
    );
    assert_eq!(
        wide.rows,
        vec![vec![
            Cell::Text("Q1".into()),
            Cell::Text("Total".into()),
            Cell::Num(1350.5),
            Cell::Num(325.5),
            Cell::Num(1025.0)
        ]]
    );
    let csv = String::from_utf8(to_csv(&wide, b';', ',').unwrap()).unwrap();
    assert_eq!(
        csv,
        "years;regions;Sales;Cost;Profit\nQ1;Total;1350,5;325,5;1025\n"
    );
    assert!(
        export_table(
            &c,
            &ExportSpec {
                area: vec![("nope".into(), vec![])],
                ..Default::default()
            },
            None
        )
        .is_err()
    );
}

#[test]
fn export_with_rules() {
    let mut c = cube();
    run(&mut c, &spec(WIDE), FILE, false);
    c.set_rules("[measures:'Profit'] = ['Sales'] - ['Cost'];")
        .unwrap();
    let t = export_table(
        &c,
        &ExportSpec {
            with_rules: true,
            aggregates: true,
            area: vec![
                ("years".into(), vec!["Jan".into()]),
                ("regions".into(), vec!["Nord".into()]),
                ("measures".into(), vec!["Profit".into()]),
            ],
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(t.rows[0][3], Cell::Num(900.5));
}

proptest! {
    /// Loading any generated file twice with replace_area equals loading it once.
    #[test]
    fn replace_area_is_idempotent(rows in proptest::collection::vec((0usize..4, 0usize..2, 1u32..1000, proptest::option::of(1u32..1000)), 1..30)) {
        let months = ["Jan", "Feb", "Apr", "May"];
        let regions = ["Nord", "Sued"];
        let mut file = String::from("Monat;Region;Umsatz;Kosten\n");
        for (m, r, s, k) in rows {
            file.push_str(&format!("{};{};{};{}\n", months[m], regions[r], s, k.map_or(String::new(), |k| k.to_string())));
        }
        let s = spec(WIDE);
        let mut once = cube();
        run(&mut once, &s, &file, false);
        let mut twice = cube();
        run(&mut twice, &s, &file, false);
        run(&mut twice, &s, &file, false);
        prop_assert_eq!(snapshot(&once), snapshot(&twice));
    }
}

// ---- persistent import (creating members, journal, specs in the model) ----

fn db_with_cube(dir: &std::path::Path) -> Database {
    let (y, r, m) = dims();
    let mut db = Database::create(dir.join("p.neolap")).unwrap();
    for d in [y, r, m] {
        db.save_dimension(d, "admin").unwrap();
    }
    db.create_cube("sales", &["years", "regions", "measures"], "admin")
        .unwrap();
    db
}

#[test]
fn create_under_adds_members_to_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db_with_cube(dir.path());
    let s = spec(&WIDE.replace("reject", "create_under:Total"));
    let data = "Monat;Region;Umsatz;Kosten\nJan;Nord;1;\nJan;Mitte;2;\nJan;West;3;\n";
    let dry = db
        .run_import(&s, SourceKind::Csv, data.as_bytes(), &ctx(), true)
        .unwrap();
    assert_eq!(dry.members_created.len(), 2);
    assert!(db.dimension("regions").unwrap().resolve("Mitte").is_none()); // dry run: nothing created
    let r = db
        .run_import(&s, SourceKind::Csv, data.as_bytes(), &ctx(), false)
        .unwrap();
    assert!(r.failed.is_none() && r.cells_created == 3, "{r:?}");
    drop(db);
    let db = Database::open(dir.path().join("p.neolap")).unwrap();
    let cube = db.cube("sales").unwrap();
    assert_eq!(
        cube.value(&cube.address(&["Jan", "West", "Sales"]).unwrap())
            .unwrap(),
        Some(3.0)
    );
    assert_eq!(
        cube.value(&cube.address(&["Jan", "Total", "Sales"]).unwrap())
            .unwrap(),
        Some(6.0)
    );
}

#[test]
fn create_under_refuses_a_parent_that_holds_data() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db_with_cube(dir.path());
    let s = spec(&WIDE.replace("reject", "create_under:Nord"));
    db.run_import(&spec(WIDE), SourceKind::Csv, FILE.as_bytes(), &ctx(), false)
        .unwrap();
    let r = db
        .run_import(
            &s,
            SourceKind::Csv,
            "Monat;Region;Umsatz;Kosten\nJan;Neu;1;\n".as_bytes(),
            &ctx(),
            false,
        )
        .unwrap();
    assert!(r.failed.as_deref().unwrap().contains("holds data"));
    assert!(db.dimension("regions").unwrap().resolve("Neu").is_none());
    let r = db
        .run_import(
            &spec(&WIDE.replace("reject", "create_under:Gibtsnicht")),
            SourceKind::Csv,
            "Monat;Region;Umsatz;Kosten\nJan;Neu;1;\n".as_bytes(),
            &ctx(),
            false,
        )
        .unwrap();
    assert!(r.failed.as_deref().unwrap().contains("does not exist"));
}

#[test]
fn imports_are_journaled_undoable_and_specs_are_stored() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db_with_cube(dir.path());
    db.save_import_spec(WIDE, "admin").unwrap();
    assert_eq!(db.import_spec_names().unwrap(), ["Ist-Umsatz laden"]);
    let s = ImportSpec::from_toml(&db.import_spec("ist-umsatz laden").unwrap()).unwrap();
    let r = db
        .run_import(&s, SourceKind::Csv, FILE.as_bytes(), &ctx(), false)
        .unwrap();
    let entries = db
        .journal(&crate::persist::JournalQuery {
            user: Some("anna".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!((entries[0].op.as_str(), entries[0].n_cells), ("import", 5));
    assert_eq!(r.op_id, Some(0));
    db.cube_mut("sales").unwrap().undo(&ctx()).unwrap();
    assert_eq!(db.stored_cells("sales").unwrap(), 0);
    assert!(
        db.save_import_spec(&WIDE.replace("\"sales\"", "\"nope\""), "a")
            .is_err()
    );
    assert!(db.import_spec("none").is_err());
}

#[test]
fn schema_v1_files_are_migrated_with_a_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.neolap");
    {
        let db = db_with_cube(dir.path());
        drop(db);
        let conn = rusqlite::Connection::open(dir.path().join("p.neolap")).unwrap();
        conn.execute_batch("DROP TABLE import_specs; DROP TABLE tokens;")
            .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "UPDATE meta SET value = '1' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
        std::fs::copy(dir.path().join("p.neolap"), &path).ok();
        drop(conn);
        std::fs::remove_file(dir.path().join("p.neolap-wal")).ok();
        std::fs::remove_file(dir.path().join("p.neolap-shm")).ok();
        std::fs::copy(dir.path().join("p.neolap"), &path).unwrap();
    }
    let mut db = Database::open(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), crate::persist::SCHEMA_VERSION);
    let backup = format!("old.neolap.before-v{}", crate::persist::SCHEMA_VERSION);
    assert!(dir.path().join(backup).exists());
    db.save_import_spec(WIDE, "a").unwrap();
    assert!(db.cube("sales").is_some());
}

struct Cubes(Vec<Cube>);
impl CubeProvider for Cubes {
    fn cube(&self, name: &str) -> Option<&Cube> {
        self.0.iter().find(|c| c.name() == name)
    }
}

#[test]
fn export_can_read_other_cubes_through_rules() {
    let c = cube();
    let none = Cubes(Vec::new());
    assert!(
        export_table(&c, &ExportSpec::default(), Some(&none))
            .unwrap()
            .rows
            .is_empty()
    );
}

/// M6 target: import of 1 million CSV rows in < 10 s.
/// `cargo test --release -p neolap-core -- --ignored --nocapture import_1m`
#[test]
#[ignore]
fn import_1m() {
    use std::time::Instant;
    let mut e = Dimension::new("a", DimensionKind::Generic).edit();
    let root = e.add("All").unwrap();
    for i in 0..1000 {
        let m = e.add(&format!("a{i}")).unwrap();
        e.add_edge(root, m, 1.0).unwrap();
    }
    let a = Arc::new(e.commit().unwrap());
    let mut e = Dimension::new("m", DimensionKind::Measures).edit();
    for n in ["x", "y"] {
        e.add(n).unwrap();
    }
    let m = Arc::new(e.commit().unwrap());
    let mut e = Dimension::new("b", DimensionKind::Generic).edit();
    let root = e.add("All").unwrap();
    for i in 0..1000 {
        let k = e.add(&format!("b{i}")).unwrap();
        e.add_edge(root, k, 1.0).unwrap();
    }
    let b = Arc::new(e.commit().unwrap());
    let mut cube = Cube::new("c", vec![a, b, m]);
    let mut file = String::from("a,b,x,y\n");
    for i in 0..1_000_000u64 {
        file.push_str(&format!(
            "a{},b{},{}.5,{}\n",
            i % 1000,
            (i / 1000) % 1000,
            i % 97 + 1,
            i % 13 + 1
        ));
    }
    let s = spec(
        "name='big'\ncube='c'\n[map]\na='a'\nb='b'\n[values]\nx='x'\ny='y'\n[options]\nmode='replace_area'\n",
    );
    let t = Instant::now();
    let r = run_import(
        &mut cube,
        &s,
        SourceKind::Csv,
        file.as_bytes(),
        &WriteContext::new("u"),
        false,
        None,
    )
    .unwrap();
    let el = t.elapsed();
    println!("1,000,000 rows → {} cells: {el:?}", r.cells_created);
    assert!(el.as_secs() < 10, "too slow: {el:?}");
    let t = Instant::now();
    let r = run_import(
        &mut cube,
        &s,
        SourceKind::Csv,
        file.as_bytes(),
        &WriteContext::new("u"),
        false,
        None,
    )
    .unwrap();
    println!(
        "second run (replace_area, unchanged): {:?}, changed {}",
        t.elapsed(),
        r.cells_created + r.cells_updated + r.cells_deleted
    );
}

/// M6 target through the database: 1 million rows including file write and journal.
/// `cargo test --release -p neolap-core -- --ignored --nocapture import_1m_persistent`
#[test]
#[ignore]
fn import_1m_persistent() {
    use std::time::Instant;
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::create(dir.path().join("big.neolap")).unwrap();
    for (name, kind, n) in [
        ("a", DimensionKind::Generic, 1000),
        ("b", DimensionKind::Generic, 1000),
        ("m", DimensionKind::Measures, 2),
    ] {
        let mut e = Dimension::new(name, kind).edit();
        let root = (n > 2).then(|| e.add("All").unwrap());
        for i in 0..n {
            let id = e
                .add(&if n > 2 {
                    format!("{name}{i}")
                } else {
                    ["x", "y"][i].to_string()
                })
                .unwrap();
            if let Some(r) = root {
                e.add_edge(r, id, 1.0).unwrap();
            }
        }
        db.save_dimension(e.commit().unwrap(), "u").unwrap();
    }
    db.create_cube("c", &["a", "b", "m"], "u").unwrap();
    let mut file = String::from("a,b,x,y\n");
    for i in 0..1_000_000u64 {
        file.push_str(&format!(
            "a{},b{},{}.5,{}\n",
            i % 1000,
            (i / 1000) % 1000,
            i % 97 + 1,
            i % 13 + 1
        ));
    }
    let s = spec(
        "name='big'\ncube='c'\n[map]\na='a'\nb='b'\n[values]\nx='x'\ny='y'\n[options]\nmode='replace_area'\n",
    );
    let t = Instant::now();
    let r = db
        .run_import(
            &s,
            SourceKind::Csv,
            file.as_bytes(),
            &WriteContext::new("u"),
            false,
        )
        .unwrap();
    let el = t.elapsed();
    println!(
        "1,000,000 rows → {} cells incl. file + journal: {el:?}",
        r.cells_created
    );
    assert!(el.as_secs() < 10, "too slow: {el:?}");
    let t = Instant::now();
    drop(db);
    let db = Database::open(dir.path().join("big.neolap")).unwrap();
    println!(
        "reopen: {:?}, cells {}",
        t.elapsed(),
        db.cube("c").unwrap().n_cells()
    );
}
