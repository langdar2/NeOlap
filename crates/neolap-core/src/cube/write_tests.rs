use std::sync::Arc;

use proptest::prelude::*;

use super::tests::{random_tree, tiny, val};
use super::*;
use crate::Error;
use crate::dimension::{Dimension, DimensionKind};

fn ctx() -> WriteContext {
    WriteContext::new("anna")
}

fn put(c: &mut Cube, ctx: &WriteContext, a: &[&str], input: &str) -> Result<WriteResult> {
    let addr = c.address(a).unwrap();
    c.write(ctx, &addr, Input::parse(input).unwrap())
}

#[test]
fn input_syntax() {
    assert_eq!(Input::parse("").unwrap(), Input::Clear);
    assert_eq!(Input::parse("500").unwrap(), Input::Set(500.0));
    assert_eq!(Input::parse("=-50").unwrap(), Input::Set(-50.0));
    assert_eq!(Input::parse("#500").unwrap(), Input::Equal(500.0));
    assert_eq!(Input::parse("+50").unwrap(), Input::Delta(50.0));
    assert_eq!(Input::parse("-50").unwrap(), Input::Delta(-50.0));
    assert_eq!(Input::parse("+5%").unwrap(), Input::Percent(5.0));
    assert_eq!(Input::parse("1.234,5").unwrap(), Input::Set(1234.5));
    assert_eq!(Input::parse("1,234.5").unwrap(), Input::Set(1234.5));
    assert_eq!(
        Input::parse("@years:Q2,Sales:90").unwrap(),
        Input::Reference {
            members: vec!["years:Q2".into(), "Sales".into()],
            target: 90.0
        }
    );
    assert!(Input::parse("abc").is_err());
    assert!(Input::parse("@Q2").is_err());
}

#[test]
fn proportional_keeps_structure_and_hits_target() {
    let mut c = tiny();
    // Q1 Sales: Jan 100, Feb 50 → 300
    let r = put(&mut c, &ctx(), &["Q1", "Sales"], "300").unwrap();
    assert_eq!(r.cells_changed, 2);
    assert_eq!(val(&c, &["Jan", "Sales"]), Some(200.0));
    assert_eq!(val(&c, &["Feb", "Sales"]), Some(100.0));
    assert_eq!(val(&c, &["Q1", "Sales"]), Some(300.0));
}

#[test]
fn proportional_falls_back_to_equal() {
    let mut c = tiny();
    // Q2 Cost is empty → equal over Apr only; Total Cost has values, so use an empty area instead
    put(&mut c, &ctx(), &["Q2", "Cost"], "40").unwrap();
    assert_eq!(val(&c, &["Apr", "Cost"]), Some(40.0));
    // an empty area with several cells: Q1 × Profit has weights → rejected, use a fresh cube area
    let mut c = tiny();
    put(&mut c, &ctx(), &["Q1", "Sales"], "").unwrap();
    put(&mut c, &ctx(), &["Q1", "Sales"], "101").unwrap();
    assert_eq!(val(&c, &["Jan", "Sales"]), Some(50.5));
    assert_eq!(val(&c, &["Feb", "Sales"]), Some(50.5));
}

#[test]
fn equal_delta_percent_and_clear() {
    let mut c = tiny();
    put(&mut c, &ctx(), &["Q1", "Sales"], "#90").unwrap();
    assert_eq!(val(&c, &["Jan", "Sales"]), Some(45.0));
    put(&mut c, &ctx(), &["Q1", "Sales"], "+30").unwrap(); // 90 → 120, still equal
    assert_eq!(val(&c, &["Feb", "Sales"]), Some(60.0));
    put(&mut c, &ctx(), &["Q1", "Sales"], "-50%").unwrap();
    assert_eq!(val(&c, &["Q1", "Sales"]), Some(60.0));
    put(&mut c, &ctx(), &["Q1", "Sales"], "").unwrap();
    assert_eq!(val(&c, &["Q1", "Sales"]), None);
    assert_eq!(val(&c, &["Total", "Sales"]), Some(10.0));
}

#[test]
fn zero_deletes_cells() {
    let mut c = tiny();
    let n = c.n_cells();
    put(&mut c, &ctx(), &["Jan", "Sales"], "0").unwrap();
    assert_eq!(c.n_cells(), n - 1);
    assert_eq!(val(&c, &["Jan", "Sales"]), None);
}

#[test]
fn reference_distribution() {
    let mut c = tiny();
    // distribute 90 onto Q2 Cost following the structure of Q1 Sales? needs equal size → use measures:
    // Jan/Feb Cost (30, 20) as reference for Jan/Feb Sales: dimension "measures" → Sales := Cost structure
    put(&mut c, &ctx(), &["Q1", "Sales"], "@measures:Cost:100").unwrap();
    assert_eq!(val(&c, &["Jan", "Sales"]), Some(60.0));
    assert_eq!(val(&c, &["Feb", "Sales"]), Some(40.0));
}

#[test]
fn weighted_target_is_rejected() {
    let mut c = tiny();
    let before = c.generation();
    assert!(matches!(
        put(&mut c, &ctx(), &["Q1", "Profit"], "10"),
        Err(Error::InvalidOperation(_))
    ));
    assert_eq!(c.generation(), before);
}

#[test]
fn equal_cell_limit_with_preview() {
    let mut c = tiny();
    let addr = c.address(&["Total", "Sales"]).unwrap();
    assert_eq!(c.area_info(&addr).unwrap().base_cells, 3);
    let mut limited = ctx();
    limited.max_equal_cells = 2;
    let err = c.write(&limited, &addr, Input::Equal(30.0)).unwrap_err();
    assert!(err.to_string().contains("limit"));
    assert_eq!(val(&c, &["Total", "Sales"]), Some(160.0));
}

#[test]
fn permissions_reject_the_whole_operation() {
    let mut c = tiny();
    let jan = c.dimensions()[0]
        .leaf_ordinal(c.dimensions()[0].member("Jan").unwrap())
        .unwrap();
    let mut limited = ctx();
    limited.can_write = Some(Arc::new(move |coords| coords[0] != jan));
    let err = put(&mut c, &limited, &["Q1", "Sales"], "300").unwrap_err();
    assert!(matches!(err, Error::Denied(m) if m.contains("Jan")));
    assert_eq!(val(&c, &["Q1", "Sales"]), Some(150.0));
}

#[test]
fn conflict_detection_names_the_other_user() {
    let mut c = tiny();
    let seen = c.generation();
    put(&mut c, &WriteContext::new("ben"), &["Jan", "Sales"], "999").unwrap();
    // anna works on a view from before → affected area changed
    let err = put(&mut c, &ctx().expecting(seen), &["Q1", "Sales"], "300").unwrap_err();
    match err {
        Error::WriteConflict { user, cell, value } => {
            assert_eq!(user, "ben");
            assert!(cell.contains("Jan"));
            assert_eq!(value, Some(999.0));
        }
        e => panic!("unexpected {e}"),
    }
    // an unrelated area is fine
    put(&mut c, &ctx().expecting(seen), &["Apr", "Sales"], "11").unwrap();
    // with the current generation, overwriting is possible
    let now = c.generation();
    put(&mut c, &ctx().expecting(now), &["Q1", "Sales"], "300").unwrap();
}

#[test]
fn undo_restores_state_and_stack_is_per_user() {
    let mut c = tiny();
    let snapshot: Vec<_> = {
        let mut v: Vec<_> = c.store().iter().collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    put(&mut c, &ctx(), &["Q1", "Sales"], "300").unwrap();
    put(&mut c, &WriteContext::new("ben"), &["Apr", "Sales"], "12").unwrap();
    // anna's operation touched Jan/Feb Sales only, ben's Apr → undo is allowed
    c.undo(&ctx()).unwrap();
    assert_eq!(val(&c, &["Q1", "Sales"]), Some(150.0));
    assert_eq!(val(&c, &["Apr", "Sales"]), Some(12.0));
    c.undo(&WriteContext::new("ben")).unwrap();
    let mut now: Vec<_> = c.store().iter().collect();
    now.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(now, snapshot);
    assert!(c.undo(&ctx()).is_err());
}

#[test]
fn undo_is_rejected_after_foreign_change() {
    let mut c = tiny();
    put(&mut c, &ctx(), &["Q1", "Sales"], "300").unwrap();
    put(&mut c, &WriteContext::new("ben"), &["Jan", "Sales"], "1").unwrap();
    let err = c.undo(&ctx()).unwrap_err();
    assert!(matches!(err, Error::WriteConflict { ref user, .. } if user == "ben"));
    // still possible after ben undoes his change
    c.undo(&WriteContext::new("ben")).unwrap();
    c.undo(&ctx()).unwrap();
    assert_eq!(val(&c, &["Q1", "Sales"]), Some(150.0));
}

#[test]
fn cell_versions_and_undo_depth() {
    let mut c = tiny();
    let jan = c.address(&["Jan", "Sales"]).unwrap();
    assert!(c.cell_version(&jan).unwrap().is_some()); // loaded by "system"
    let r = put(&mut c, &WriteContext::new("ben"), &["Jan", "Sales"], "5").unwrap();
    let v = c.cell_version(&jan).unwrap().unwrap();
    assert_eq!((v.user.as_str(), v.generation), ("ben", r.generation));
    for i in 0..UNDO_DEPTH + 10 {
        put(&mut c, &ctx(), &["Apr", "Sales"], &format!("{}", i + 1)).unwrap();
    }
    for _ in 0..UNDO_DEPTH {
        c.undo(&ctx()).unwrap();
    }
    assert!(c.undo(&ctx()).is_err());
}

#[test]
fn unchanged_write_records_nothing() {
    let mut c = tiny();
    let g = c.generation();
    let r = put(&mut c, &ctx(), &["Jan", "Sales"], "100").unwrap();
    assert_eq!((r.op_id, r.cells_changed, c.generation()), (None, 0, g));
}

#[test]
fn rounding_difference_goes_to_the_largest_cell() {
    let mut c = tiny();
    let mut r = ctx();
    r.decimals = Some(2);
    // 100 : 50 → 33.333… and 66.666… ; target 100 must be exact
    put(&mut c, &r, &["Q1", "Sales"], "100").unwrap();
    assert_eq!(val(&c, &["Q1", "Sales"]), Some(100.0));
    assert_eq!(val(&c, &["Jan", "Sales"]), Some(66.67));
}

fn splash_cube(pa: &[usize], pb: &[usize], w: &[i8]) -> (Cube, Vec<Arc<Dimension>>) {
    // weights must be 1 for splashing → use a weight vector of ones
    let _ = w;
    let ones = [0i8];
    let dims: Vec<Arc<Dimension>> = vec![
        Arc::new(random_tree("a", pa, &ones)),
        Arc::new(random_tree("b", pb, &ones)),
    ];
    let _ = DimensionKind::Generic;
    (Cube::new("p", dims.clone()), dims)
}

proptest! {
    /// Target exact (to rounding), proportions kept, undo restores the previous state.
    #[test]
    fn splash_properties(
        pa in proptest::collection::vec(0usize..100, 1..20),
        pb in proptest::collection::vec(0usize..100, 1..20),
        facts in proptest::collection::vec((any::<u16>(), any::<u16>(), 1u32..1000), 1..60),
        target in 1u32..100_000,
        pick in (any::<u16>(), any::<u16>()),
        mode in 0usize..4,
    ) {
        let (mut cube, dims) = splash_cube(&pa, &pb, &[0]);
        for (x, y, v) in facts {
            let a = dims[0].leaf_member(x as u32 % dims[0].n_leaves()).unwrap();
            let b = dims[1].leaf_member(y as u32 % dims[1].n_leaves()).unwrap();
            cube.set_base(&[a, b], Some(f64::from(v))).unwrap();
        }
        let ma = pick.0 as u32 % dims[0].len() as u32;
        let mb = pick.1 as u32 % dims[1].len() as u32;
        let before: std::collections::BTreeMap<_, _> =
            cube.store().iter().map(|(k, v)| (k, v.to_bits())).collect();
        let area_before = cube.value(&[ma, mb]).unwrap();
        let cells_before: std::collections::BTreeMap<Vec<u32>, f64> = cube.store().iter().collect();
        let mut c = WriteContext::new("u");
        c.decimals = Some(2);
        let t = f64::from(target);
        let input = match mode { 0 => Input::Set(t), 1 => Input::Equal(t), 2 => Input::Delta(t), _ => Input::Percent(10.0) };
        let expected = match mode {
            0 | 1 => Some(t),
            2 => Some(area_before.unwrap_or(0.0) + t),
            _ => None,
        };
        let result = cube.write(&c, &[ma, mb], input.clone()).unwrap();
        let got = cube.value(&[ma, mb]).unwrap();
        if let Some(e) = expected {
            prop_assert!((got.unwrap_or(0.0) - e).abs() < 1e-6, "target {e}, got {got:?}");
        }
        if mode == 0 && area_before.is_some_and(|s| s.abs() > 1e-9) {
            // proportions between cells that existed before are kept (up to rounding to 2 decimals)
            let after: std::collections::BTreeMap<Vec<u32>, f64> = cube.store().iter().collect();
            let f = t / area_before.unwrap();
            let (sa, sb) = (dims[0].leaf_set(ma), dims[1].leaf_set(mb));
            for (k, v) in cells_before.iter().filter(|(k, _)| sa.contains(k[0]) && sb.contains(k[1])) {
                if let Some(n) = after.get(k) {
                    prop_assert!((n - v * f).abs() < 0.02 + v * f * 1e-9 + 0.005 * cells_before.len() as f64);
                }
            }
        }
        if result.op_id.is_some() {
            cube.undo(&c).unwrap();
        } else {
            prop_assert!(cube.undo(&c).is_err());
        }
        let after_undo: std::collections::BTreeMap<_, _> =
            cube.store().iter().map(|(k, v)| (k, v.to_bits())).collect();
        prop_assert_eq!(before, after_undo);
    }
}

/// M3 benchmark: splash onto 100,000 base cells including storing.
/// `cargo test --release -p neolap-core -- --ignored --nocapture splash_100k`
#[test]
#[ignore]
fn splash_100k() {
    use std::time::Instant;
    let mut e = Dimension::new("a", DimensionKind::Generic).edit();
    let root = e.add("All").unwrap();
    for i in 0..1000 {
        let m = e.add(&format!("a{i}")).unwrap();
        e.add_edge(root, m, 1.0).unwrap();
    }
    let a = Arc::new(e.commit().unwrap());
    let mut e = Dimension::new("b", DimensionKind::Generic).edit();
    let root = e.add("All").unwrap();
    for i in 0..100 {
        let m = e.add(&format!("b{i}")).unwrap();
        e.add_edge(root, m, 1.0).unwrap();
    }
    let b = Arc::new(e.commit().unwrap());
    let mut cube = Cube::new("s", vec![a.clone(), b.clone()]);
    let all = [a.member("All").unwrap(), b.member("All").unwrap()];
    let c = WriteContext::new("u");
    let t = Instant::now();
    let r = cube.write(&c, &all, Input::Equal(1_000_000.0)).unwrap();
    println!("equal splash {} cells: {:?}", r.cells_changed, t.elapsed());
    let t = Instant::now();
    let r = cube.write(&c, &all, Input::Set(2_000_000.0)).unwrap();
    println!(
        "proportional splash {} cells: {:?}",
        r.cells_changed,
        t.elapsed()
    );
    let t = Instant::now();
    cube.undo(&c).unwrap();
    println!("undo: {:?}", t.elapsed());
    assert_eq!(cube.value(&all).unwrap(), Some(1_000_000.0));
}
