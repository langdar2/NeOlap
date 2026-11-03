use super::*;
use crate::Error;
use proptest::prelude::*;

fn dim(f: impl FnOnce(&mut DimensionEditor)) -> Dimension {
    let mut ed = Dimension::new("d", DimensionKind::Generic).edit();
    f(&mut ed);
    ed.commit().unwrap()
}

#[test]
fn hierarchy_levels_and_leaf_ranges() {
    let d = dim(|e| {
        let total = e.add("Total").unwrap();
        let q1 = e.add("Q1").unwrap();
        let q2 = e.add("Q2").unwrap();
        e.add_edge(total, q1, 1.0).unwrap();
        e.add_edge(total, q2, 1.0).unwrap();
        e.add_children(q1, &["Jan", "Feb"]).unwrap();
        e.add_children(q2, &["Apr"]).unwrap();
    });
    let total = d.member("total").unwrap();
    assert_eq!(d.level(total), 2);
    assert_eq!(d.level(d.member("Q1").unwrap()), 1);
    assert_eq!(d.depth(d.member("Jan").unwrap()), 2);
    assert_eq!(d.n_leaves(), 3);
    let set = d.leaf_set(total);
    assert_eq!(set.as_range(), Some(0..3));
    assert_eq!(d.leaf_set(d.member("Q2").unwrap()).as_range(), Some(2..3));
    assert_eq!(d.leaf_member(0), d.resolve("jan"));
}

#[test]
fn shared_leaf_counts_once() {
    // T = A + B, A = {x, y}, B = {x, z}  → 3 leaves (checked against the original)
    let d = dim(|e| {
        let t = e.add("T").unwrap();
        let a = e.add("A").unwrap();
        let b = e.add("B").unwrap();
        e.add_edge(t, a, 1.0).unwrap();
        e.add_edge(t, b, 1.0).unwrap();
        e.add_children(a, &["x", "y"]).unwrap();
        e.add_children(b, &["x", "z"]).unwrap();
    });
    assert_eq!(d.leaf_set(d.member("T").unwrap()).len(), 3);
    // B is not contiguous in DFS order (x, y, z) → x is first, so B = {x, z} is a bit set.
    assert!(matches!(
        d.leaf_set(d.member("B").unwrap()).shape(),
        LeafShape::Bits { .. }
    ));
}

#[test]
fn contradicting_path_weights_are_rejected() {
    let mut e = Dimension::new("d", DimensionKind::Generic).edit();
    let t = e.add("T").unwrap();
    let a = e.add("A").unwrap();
    let b = e.add("B").unwrap();
    let x = e.add("x").unwrap();
    e.add_edge(t, a, 1.0).unwrap();
    e.add_edge(t, b, 1.0).unwrap();
    e.add_edge(a, x, 1.0).unwrap();
    e.add_edge(b, x, -1.0).unwrap();
    assert!(matches!(e.commit(), Err(Error::Conflict(_))));
}

#[test]
fn weights_are_applied_in_leaf_set() {
    let d = dim(|e| {
        let p = e.add("Profit").unwrap();
        let s = e.add("Sales").unwrap();
        let c = e.add("Cost").unwrap();
        e.add_edge(p, s, 1.0).unwrap();
        e.add_edge(p, c, -1.0).unwrap();
    });
    let set = d.leaf_set(d.member("Profit").unwrap());
    let cost = d.leaf_ordinal(d.member("Cost").unwrap()).unwrap();
    assert_eq!(set.weight(cost), -1.0);
    assert!(set.has_weights());
}

#[test]
fn cycles_are_rejected() {
    let mut e = Dimension::new("d", DimensionKind::Generic).edit();
    let a = e.add("A").unwrap();
    let b = e.add("B").unwrap();
    e.add_edge(a, b, 1.0).unwrap();
    e.add_edge(b, a, 1.0).unwrap();
    assert!(matches!(e.commit(), Err(Error::Cycle { .. })));
}

#[test]
fn names_and_aliases_are_case_insensitive_and_unique() {
    let mut e = Dimension::new("d", DimensionKind::Generic).edit();
    let a = e.add("Alpha").unwrap();
    let b = e.add("Beta").unwrap();
    assert!(e.add("ALPHA").is_err());
    e.set_alias(a, "A").unwrap();
    assert!(e.set_alias(b, "a").is_err());
    assert!(e.rename(b, "alpha").is_err());
    e.rename(b, "Gamma").unwrap();
    let d = e.commit().unwrap();
    assert_eq!(d.resolve("a"), Some(a));
    assert_eq!(d.resolve("beta"), None);
    assert_eq!(d.member_name(b), Some("Gamma"));
}

#[test]
fn remove_reuses_slot_and_move_keeps_weight() {
    let mut e = Dimension::new("d", DimensionKind::Generic).edit();
    let t = e.add("T").unwrap();
    let u = e.add("U").unwrap();
    let x = e.add("x").unwrap();
    e.add_edge(t, x, -1.0).unwrap();
    e.move_member(x, Some(t), Some(u)).unwrap();
    let y = e.add("y").unwrap();
    e.remove(y).unwrap();
    let z = e.add("z").unwrap();
    assert_eq!(z, y);
    let d = e.commit().unwrap();
    assert_eq!(d.children(u), &[(x, -1.0)]);
    assert!(d.is_leaf(t));
}

#[test]
fn search_prefers_prefix_and_finds_aliases() {
    let d = dim(|e| {
        let a = e.add("Nordamerika").unwrap();
        e.add("Europa Nord").unwrap();
        e.set_alias(a, "NA").unwrap();
        e.add("Süd").unwrap();
    });
    let hits: Vec<_> = d
        .search("nord", 10)
        .iter()
        .map(|&i| d.member_name(i).unwrap().to_string())
        .collect();
    assert_eq!(hits, ["Nordamerika", "Europa Nord"]);
    assert_eq!(d.search("na", 10), vec![d.member("Nordamerika").unwrap()]); // found via the alias "NA"
}

#[test]
fn attributes_and_subsets_survive_edit_roundtrip() {
    let d = dim(|e| {
        let a = e.add("DE").unwrap();
        e.set_attribute("currency", a, AttrValue::Text("EUR".into()))
            .unwrap();
        e.set_subset("core", vec![a]).unwrap();
    });
    let d2 = d.edit().commit().unwrap();
    let id = d2.member("de").unwrap();
    assert_eq!(
        d2.attribute("currency", id),
        Some(&AttrValue::Text("EUR".into()))
    );
    assert_eq!(d2.subsets()[0].members, vec![id]);
}

#[test]
fn import_parent_child_with_weights_and_attributes() {
    let csv = "child;parent;weight;currency\nNord;Total;;EUR\nSüd;Total;;CHF\nCost;Profit;-1,0;\nSales;Profit;;\n";
    let mut e = Dimension::new("d", DimensionKind::Generic).edit();
    let r = import_parent_child(&mut e, csv.as_bytes(), b';', ImportMode::AddOnly).unwrap();
    assert_eq!((r.rows, r.members_added), (4, 6));
    let d = e.commit().unwrap();
    let profit = d.member("Profit").unwrap();
    assert_eq!(d.children(profit)[0], (d.member("Cost").unwrap(), -1.0));
    assert_eq!(
        d.attribute("currency", d.member("Süd").unwrap()),
        Some(&AttrValue::Text("CHF".into()))
    );
}

#[test]
fn import_levels_and_sync_removal() {
    let mut e = Dimension::new("d", DimensionKind::Generic).edit();
    import_levels(
        &mut e,
        "L1;L2;L3\nAll;EU;DE\nAll;EU;FR\nAll;US;\n".as_bytes(),
        b';',
        ImportMode::AddOnly,
    )
    .unwrap();
    let d = e.commit().unwrap();
    assert_eq!(d.n_leaves(), 3); // DE, FR, US
    let mut e = d.edit();
    let r = import_levels(
        &mut e,
        "L1;L2;L3\nAll;EU;DE\n".as_bytes(),
        b';',
        ImportMode::Sync,
    )
    .unwrap();
    assert_eq!(r.removed.len(), 2); // FR, US
    let d = e.commit().unwrap();
    assert!(d.resolve("fr").is_none());
}

#[test]
fn import_reports_bad_rows() {
    let mut e = Dimension::new("d", DimensionKind::Generic).edit();
    let err = import_parent_child(
        &mut e,
        "child;parent;weight\nA;B;x\n".as_bytes(),
        b';',
        ImportMode::AddOnly,
    )
    .unwrap_err();
    assert!(err.to_string().contains("line 2"));
}

proptest! {
    /// Random trees: every aggregate's leaf set equals the DFS range and its size matches a naive count.
    #[test]
    fn tree_leaf_sets_are_ranges(parents in proptest::collection::vec(0usize..1000, 1..200)) {
        let mut e = Dimension::new("d", DimensionKind::Generic).edit();
        let mut ids = vec![e.add("m0").unwrap()];
        for (i, p) in parents.iter().enumerate() {
            let id = e.add(&format!("m{}", i + 1)).unwrap();
            e.add_edge(ids[p % ids.len()], id, 1.0).unwrap();
            ids.push(id);
        }
        let d = e.commit().unwrap();
        for &m in &ids {
            let set = d.leaf_set(m);
            prop_assert!(set.as_range().is_some());
            // naive count
            let mut stack = vec![m];
            let mut n = 0;
            while let Some(v) = stack.pop() {
                if d.children(v).is_empty() { n += 1 } else { stack.extend(d.children(v).iter().map(|c| c.0)) }
            }
            prop_assert_eq!(set.len(), n);
        }
        prop_assert_eq!(d.leaf_set(ids[0]).len() as u32, d.n_leaves());
    }
}

/// Performance target of M1 (500,000 members, 50 levels): import < 5 s, commit < 1 s.
/// Run with `cargo test --release -p neolap-core -- --ignored --nocapture`.
#[test]
#[ignore]
fn large_dimension_meets_targets() {
    use std::time::Instant;
    let n = 500_000usize;
    let mut csv = String::from("child;parent\n");
    csv.push_str("m0;\n");
    // a spine of 50 levels plus a bushy remainder (unbalanced tree)
    for i in 1..n {
        let parent = if i < 50 {
            i - 1
        } else {
            40 + (i * 7919) % (i.min(5000))
        };
        csv.push_str(&format!("m{i};m{parent}\n"));
    }
    let t = Instant::now();
    let mut e = Dimension::new("big", DimensionKind::Generic).edit();
    import_parent_child(&mut e, csv.as_bytes(), b';', ImportMode::AddOnly).unwrap();
    let import = t.elapsed();
    let t = Instant::now();
    let d = e.commit().unwrap();
    let commit = t.elapsed();
    println!(
        "import {import:?}, commit {commit:?}, leaves {}",
        d.n_leaves()
    );
    assert!(import.as_secs_f64() < 5.0, "import too slow: {import:?}");
    assert!(commit.as_secs_f64() < 1.0, "commit too slow: {commit:?}");
}
