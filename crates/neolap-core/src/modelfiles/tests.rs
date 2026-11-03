use std::path::Path;

use super::*;
use crate::cube::{Input, WriteContext};
use crate::dimension::{AttrValue, DimensionKind};

fn build(dir: &Path) -> Database {
    let mut db = Database::create(dir.join("a.neolap")).unwrap();
    let mut y = Dimension::new("years", DimensionKind::Time).edit();
    let total = y.add("Year").unwrap();
    for (q, months) in [("Q1", ["Jan", "Feb"]), ("Q2", ["Apr", "May"])] {
        let q = y.add(q).unwrap();
        y.add_edge(total, q, 1.0).unwrap();
        y.add_children(q, &months).unwrap();
    }
    // a second hierarchy: H1 contains Jan and Q2 (alternative parent), plus a weight
    let h1 = y.add("H1").unwrap();
    let (jan, q2) = (y.resolve("Jan").unwrap(), y.resolve("Q2").unwrap());
    y.add_edge(h1, jan, 1.0).unwrap();
    y.add_edge(h1, q2, 1.0).unwrap();
    y.set_alias(jan, "Januar").unwrap();
    y.set_alias(jan, "Jänner").unwrap();
    y.set_attribute("order", jan, AttrValue::Number(1.0))
        .unwrap();
    y.set_attribute("label", jan, AttrValue::Text("Start; \"Jahr\"".into()))
        .unwrap();
    y.set_subset("first", vec![jan]).unwrap();
    db.save_dimension(y.commit().unwrap(), "admin").unwrap();
    let mut m = Dimension::new("measures", DimensionKind::Measures).edit();
    let profit = m.add("Profit").unwrap();
    let (sales, cost) = (m.add("Sales").unwrap(), m.add("Cost").unwrap());
    m.add_edge(profit, sales, 1.0).unwrap();
    m.add_edge(profit, cost, -1.0).unwrap();
    m.add("Margin").unwrap();
    db.save_dimension(m.commit().unwrap(), "admin").unwrap();
    db.create_cube("sales", &["years", "measures"], "admin")
        .unwrap();
    db.set_rules(
        "sales",
        "# Marge\n['Margin'] = IF(['Sales'] = 0, NULL, ['Profit'] / ['Sales']);",
        "admin",
    )
    .unwrap();
    db.save_import_spec(
        "name = \"laden\"\ncube = \"sales\"\nvalue_column = \"v\"\n[map]\nyears = \"y\"\nmeasures = \"m\"\n",
        "admin",
    )
    .unwrap();
    db
}

fn tree(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push((
                    p.strip_prefix(dir).unwrap().display().to_string(),
                    std::fs::read_to_string(&p).unwrap(),
                ));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn export_is_readable_and_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let db = build(dir.path());
    let (a, b) = (dir.path().join("m1"), dir.path().join("m2"));
    export_model(&db, &a).unwrap();
    export_model(&db, &b).unwrap();
    assert_eq!(tree(&a), tree(&b));
    let files: Vec<String> = tree(&a).into_iter().map(|f| f.0).collect();
    assert_eq!(
        files,
        [
            "cubes/sales.toml",
            "dimensions/measures.csv",
            "dimensions/measures.toml",
            "dimensions/years.csv",
            "dimensions/years.toml",
            "imports/laden.toml",
            "rules/sales.rules"
        ]
    );
    let csv = std::fs::read_to_string(a.join("dimensions/years.csv")).unwrap();
    assert!(
        csv.starts_with("child;parent;weight;alias;label;order\n"),
        "{csv}"
    );
    assert!(csv.contains("Year;;;;;\n"));
    assert!(csv.contains("Jan;Q1;;Januar|Jänner;\"Start; \"\"Jahr\"\"\";1\n"));
    assert!(csv.contains("Jan;H1;;;;\n")); // second parent: one row per edge, no repeated alias
    let m = std::fs::read_to_string(a.join("dimensions/measures.csv")).unwrap();
    assert!(m.contains("Cost;Profit;-1;"));
    assert_eq!(
        std::fs::read_to_string(a.join("rules/sales.rules")).unwrap(),
        "# Marge\n['Margin'] = IF(['Sales'] = 0, NULL, ['Profit'] / ['Sales']);\n"
    );
}

#[test]
fn roundtrip_into_an_empty_database_reproduces_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let db = build(dir.path());
    let m1 = dir.path().join("m1");
    export_model(&db, &m1).unwrap();
    let mut fresh = Database::create(dir.path().join("b.neolap")).unwrap();
    let r = import_model(&mut fresh, &m1, "admin").unwrap();
    assert_eq!(r.dimensions, ["measures", "years"]);
    assert_eq!(
        (
            r.cubes.as_slice(),
            r.rules.as_slice(),
            r.import_specs.as_slice()
        ),
        (
            ["sales".to_string()].as_slice(),
            ["sales".to_string()].as_slice(),
            ["laden".to_string()].as_slice()
        )
    );
    let m2 = dir.path().join("m2");
    export_model(&fresh, &m2).unwrap();
    assert_eq!(tree(&m1), tree(&m2)); // files → database → files is the identity
    let years = fresh.dimension("years").unwrap();
    let jan = years.member("januar").unwrap();
    assert_eq!(years.aliases(jan), ["Januar", "Jänner"]);
    assert_eq!(
        years.attribute("label", jan),
        Some(&AttrValue::Text("Start; \"Jahr\"".into()))
    );
    assert_eq!(years.parents(jan).len(), 2);
    assert_eq!(years.subsets()[0].name, "first");
    assert_eq!(years.kind(), DimensionKind::Time);
    let cube = fresh.cube("sales").unwrap();
    assert!(cube.rules_text().contains("Marge"));
    assert!(cube.rule_status().iter().all(|s| s.valid));
}

#[test]
fn order_of_children_survives_the_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::create(dir.path().join("a.neolap")).unwrap();
    let mut d = Dimension::new("d", DimensionKind::Generic).edit();
    let root = d.add("R").unwrap();
    d.add_children(root, &["zeta", "alpha", "mid", "beta"])
        .unwrap();
    let g = d.add("G").unwrap();
    d.add_edge(root, g, 1.0).unwrap();
    d.add_children(g, &["y", "x"]).unwrap();
    db.save_dimension(d.commit().unwrap(), "a").unwrap();
    let before: Vec<String> = children_of(&db, "R");
    export_model(&db, &dir.path().join("m")).unwrap();
    let mut fresh = Database::create(dir.path().join("b.neolap")).unwrap();
    import_model(&mut fresh, &dir.path().join("m"), "a").unwrap();
    assert_eq!(children_of(&fresh, "R"), before);
    assert_eq!(children_of(&fresh, "G"), ["y", "x"]);
    // base members keep their order too (DFS numbering = display order)
    let (a, b) = (db.dimension("d").unwrap(), fresh.dimension("d").unwrap());
    let leaves = |d: &Dimension| {
        (0..d.n_leaves())
            .map(|o| {
                d.member_name(d.leaf_member(o).unwrap())
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(leaves(a), leaves(b));
}

fn children_of(db: &Database, name: &str) -> Vec<String> {
    let d = db.dimension("d").unwrap();
    d.children(d.member(name).unwrap())
        .iter()
        .map(|c| d.member_name(c.0).unwrap().to_string())
        .collect()
}

#[test]
fn reimporting_an_unchanged_model_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = build(dir.path());
    export_model(&db, &dir.path().join("m")).unwrap();
    let entries = db.journal(&Default::default()).unwrap().len();
    let r = import_model(&mut db, &dir.path().join("m"), "admin").unwrap();
    assert_eq!(r, ModelImportReport::default());
    assert_eq!(db.journal(&Default::default()).unwrap().len(), entries);
}

#[test]
fn import_keeps_cells_of_unchanged_members_and_journals_dropped_ones() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = build(dir.path());
    let ctx = WriteContext::new("anna");
    for (t, v) in [("Jan", "10"), ("Feb", "20"), ("Apr", "30")] {
        let cube = db.cube_mut("sales").unwrap();
        let a = cube.address(&[t, "Sales"]).unwrap();
        cube.write(&ctx, &a, Input::parse(v).unwrap()).unwrap();
    }
    export_model(&db, &dir.path().join("m")).unwrap();
    // edit the exported file: drop Apr, add Jun
    let p = dir.path().join("m/dimensions/years.csv");
    let text = std::fs::read_to_string(&p).unwrap();
    let text = text.replace("Apr;Q2;;;;\n", "Jun;Q2;;;;\n");
    std::fs::write(&p, text).unwrap();
    let r = import_model(&mut db, &dir.path().join("m"), "modeler").unwrap();
    assert_eq!(r.dimensions, ["years"]);
    let cube = db.cube("sales").unwrap();
    let v = |t: &str| cube.value(&cube.address(&[t, "Sales"]).unwrap()).unwrap();
    assert_eq!(
        (v("Jan"), v("Feb"), v("Jun")),
        (Some(10.0), Some(20.0), None)
    );
    // Apr's cell is gone from memory and from the file, and the change is journaled with its count
    assert_eq!(
        cube.value(&cube.address(&["Q2", "Sales"]).unwrap())
            .unwrap(),
        None
    );
    assert_eq!(db.stored_cells("sales").unwrap(), 2);
    let entry = &db
        .journal(&crate::persist::JournalQuery {
            user: Some("modeler".into()),
            ..Default::default()
        })
        .unwrap()[0];
    assert_eq!((entry.op.as_str(), entry.n_cells), ("dimension", 1));
}

#[test]
fn invalid_files_change_nothing_and_say_why() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = build(dir.path());
    export_model(&db, &dir.path().join("m")).unwrap();
    let before = db.journal(&Default::default()).unwrap().len();
    let m = dir.path().join("m");
    let try_with = |db: &mut Database, file: &str, old: &str, new: &str| -> String {
        let p = m.join(file);
        let original = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, original.replace(old, new)).unwrap();
        let res = import_model(db, &m, "x");
        std::fs::write(&p, original).unwrap();
        match res {
            Err(e) => e.to_string(),
            Ok(r) => panic!("{file}: '{old}' → '{new}' was accepted: {r:?}"),
        }
    };
    assert!(try_with(&mut db, "cubes/sales.toml", "\"measures\"", "\"nope\"").contains("nope"));
    assert!(
        try_with(
            &mut db,
            "cubes/sales.toml",
            "\"years\",\n    \"measures\"",
            "\"measures\",\n    \"years\""
        )
        .contains("cannot be changed")
    );
    assert!(try_with(&mut db, "rules/sales.rules", "IF(", "IF(;").contains("line 2"));
    assert!(
        try_with(&mut db, "dimensions/years.csv", "Q1;Year", "Q1;Q1")
            .to_lowercase()
            .contains("cycle")
    );
    assert!(
        try_with(&mut db, "dimensions/years.toml", "\"Jan\"", "\"Unknown\"")
            .contains("unknown member 'Unknown'")
    );
    assert!(
        try_with(&mut db, "imports/laden.toml", "\"sales\"", "\"nope\"").contains("unknown cube")
    );
    std::fs::write(m.join("rules/ghost.rules"), "['x'] = 1;").unwrap();
    assert!(
        import_model(&mut db, &m, "x")
            .unwrap_err()
            .to_string()
            .contains("no cube 'ghost'")
    );
    std::fs::remove_file(m.join("rules/ghost.rules")).unwrap();
    assert_eq!(db.journal(&Default::default()).unwrap().len(), before);
    assert!(import_model(&mut db, &dir.path().join("none"), "x").is_err());
}

#[test]
fn export_removes_files_of_deleted_objects_and_keeps_foreign_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = build(dir.path());
    let m = dir.path().join("m");
    export_model(&db, &m).unwrap();
    std::fs::write(m.join("README.md"), "notes").unwrap();
    db.set_rules("sales", "", "admin").unwrap();
    export_model(&db, &m).unwrap();
    assert!(!m.join("rules/sales.rules").exists());
    assert!(m.join("README.md").exists());
}

#[test]
fn awkward_names_are_quoted_or_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::create(dir.path().join("a.neolap")).unwrap();
    let mut d = Dimension::new("d", DimensionKind::Generic).edit();
    let r = d.add("a;b").unwrap();
    d.add_children(r, &["with \"quote\"", " padded ", "new\nline"])
        .unwrap();
    db.save_dimension(d.commit().unwrap(), "a").unwrap();
    export_model(&db, &dir.path().join("m")).unwrap();
    let mut fresh = Database::create(dir.path().join("b.neolap")).unwrap();
    import_model(&mut fresh, &dir.path().join("m"), "a").unwrap();
    let (a, b) = (db.dimension("d").unwrap(), fresh.dimension("d").unwrap());
    for name in ["a;b", "with \"quote\"", " padded ", "new\nline"] {
        assert!(b.resolve(name).is_some(), "{name:?}");
    }
    assert_eq!(dimension_csv(a), dimension_csv(b));
    let mut bad = Dimension::new("../escape", DimensionKind::Generic).edit();
    bad.add("x").unwrap();
    db.save_dimension(bad.commit().unwrap(), "a").unwrap();
    assert!(export_model(&db, &dir.path().join("m2")).is_err());
    assert!(!dir.path().join("escape.csv").exists());
}

#[test]
fn import_into_a_database_with_cells_keeps_every_cell_on_its_member() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = build(dir.path());
    let ctx = WriteContext::new("anna");
    let values = [("Jan", 1.0), ("Feb", 2.0), ("Apr", 4.0), ("May", 5.0)];
    for (t, v) in values {
        let cube = db.cube_mut("sales").unwrap();
        let a = cube.address(&[t, "Sales"]).unwrap();
        cube.write(&ctx, &a, Input::Set(v)).unwrap();
    }
    export_model(&db, &dir.path().join("m")).unwrap();
    // reorder the file: members appear in a different order than they were created, so a rebuild
    // from scratch would number them differently
    let p = dir.path().join("m/dimensions/years.csv");
    let text = std::fs::read_to_string(&p).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    let header = lines.remove(0);
    lines.reverse();
    std::fs::write(&p, format!("{header}\n{}\n", lines.join("\n"))).unwrap();
    import_model(&mut db, &dir.path().join("m"), "modeler").unwrap();
    let cube = db.cube("sales").unwrap();
    for (t, v) in values {
        let got = cube.value(&cube.address(&[t, "Sales"]).unwrap()).unwrap();
        assert_eq!(got, Some(v), "{t}");
    }
    assert_eq!(
        cube.value(&cube.address(&["Year", "Sales"]).unwrap())
            .unwrap(),
        Some(12.0)
    );
    // and the same after a restart
    drop(db);
    let db = Database::open(dir.path().join("a.neolap")).unwrap();
    let cube = db.cube("sales").unwrap();
    for (t, v) in values {
        assert_eq!(
            cube.value(&cube.address(&[t, "Sales"]).unwrap()).unwrap(),
            Some(v),
            "{t}"
        );
    }
}

#[test]
fn an_alias_may_take_the_name_of_a_member_removed_by_the_same_import() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::create(dir.path().join("a.neolap")).unwrap();
    let mut d = Dimension::new("d", DimensionKind::Generic).edit();
    let r = d.add("R").unwrap();
    d.add_children(r, &["old", "keep"]).unwrap();
    db.save_dimension(d.commit().unwrap(), "a").unwrap();
    export_model(&db, &dir.path().join("m")).unwrap();
    std::fs::write(
        dir.path().join("m/dimensions/d.csv"),
        "child;parent;weight;alias\nR;;;\nkeep;R;;old\n",
    )
    .unwrap();
    import_model(&mut db, &dir.path().join("m"), "a").unwrap();
    let d = db.dimension("d").unwrap();
    assert_eq!(d.resolve("old"), d.resolve("keep"));
}
