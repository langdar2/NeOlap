use super::*;
use crate::dimension::DimensionKind;
use proptest::prelude::*;

fn tiny() -> Cube {
    let mut years = Dimension::new("years", DimensionKind::Time).edit();
    let total = years.add("Total").unwrap();
    let q1 = years.add("Q1").unwrap();
    let q2 = years.add("Q2").unwrap();
    years.add_edge(total, q1, 1.0).unwrap();
    years.add_edge(total, q2, 1.0).unwrap();
    years.add_children(q1, &["Jan", "Feb"]).unwrap();
    years.add_children(q2, &["Apr"]).unwrap();
    let mut m = Dimension::new("measures", DimensionKind::Measures).edit();
    let profit = m.add("Profit").unwrap();
    let sales = m.add("Sales").unwrap();
    let cost = m.add("Cost").unwrap();
    m.add_edge(profit, sales, 1.0).unwrap();
    m.add_edge(profit, cost, -1.0).unwrap();
    let mut c = Cube::new(
        "tiny",
        vec![
            Arc::new(years.commit().unwrap()),
            Arc::new(m.commit().unwrap()),
        ],
    );
    for (t, s, v) in [
        ("Jan", "Sales", 100.0),
        ("Feb", "Sales", 50.0),
        ("Apr", "Sales", 10.0),
        ("Jan", "Cost", 30.0),
        ("Feb", "Cost", 20.0),
    ] {
        let a = c.address(&[t, s]).unwrap();
        c.set_base(&a, Some(v)).unwrap();
    }
    c
}

fn val(c: &Cube, a: &[&str]) -> Option<f64> {
    c.value(&c.address(a).unwrap()).unwrap()
}

#[test]
fn tiny_aggregates_and_weights() {
    let c = tiny();
    assert_eq!(val(&c, &["Jan", "Sales"]), Some(100.0));
    assert_eq!(val(&c, &["Q1", "Sales"]), Some(150.0));
    assert_eq!(val(&c, &["Total", "Sales"]), Some(160.0));
    assert_eq!(val(&c, &["Total", "Cost"]), Some(50.0));
    assert_eq!(val(&c, &["Q1", "Profit"]), Some(100.0)); // 150 - 50
    assert_eq!(val(&c, &["Apr", "Cost"]), None);
    assert_eq!(val(&c, &["Q2", "Cost"]), None);
}

#[test]
fn writes_invalidate_cache_and_bump_generation() {
    let mut c = tiny();
    assert_eq!(val(&c, &["Total", "Sales"]), Some(160.0));
    let g = c.generation();
    let a = c.address(&["Apr", "Sales"]).unwrap();
    assert_eq!(c.set_base(&a, Some(40.0)).unwrap(), Some(10.0));
    assert!(c.generation() > g);
    assert_eq!(val(&c, &["Total", "Sales"]), Some(190.0));
    c.set_base(&a, None).unwrap();
    assert_eq!(val(&c, &["Total", "Sales"]), Some(150.0));
    assert_eq!(c.n_cells(), 4);
}

#[test]
fn aggregates_cannot_be_written_as_base() {
    let mut c = tiny();
    let a = c.address(&["Q1", "Sales"]).unwrap();
    assert!(c.set_base(&a, Some(1.0)).is_err());
}

#[test]
fn view_matches_cell_queries_and_suppresses_empty() {
    let c = tiny();
    let rows: Vec<_> = ["Total", "Q1", "Q2", "Jan"]
        .iter()
        .map(|n| c.dimensions()[0].member(n).unwrap())
        .collect();
    let cols: Vec<_> = ["Sales", "Cost", "Profit"]
        .iter()
        .map(|n| c.dimensions()[1].member(n).unwrap())
        .collect();
    let fixed = c.address(&["Total", "Profit"]).unwrap();
    let g = c.view(0, &rows, 1, &cols, &fixed).unwrap();
    for (ri, &r) in rows.iter().enumerate() {
        for (ci, &k) in cols.iter().enumerate() {
            assert_eq!(
                g.get(ri, ci),
                c.value(&[r, k]).unwrap(),
                "row {ri} col {ci}"
            );
        }
    }
    assert_eq!(g.get(2, 1), None);
    assert_eq!(g.non_empty_rows(), vec![0, 1, 2, 3]);
}

/// Reference: weights of all base members below `m` in a single-parent tree.
fn naive_weights(d: &Dimension, m: MemberId) -> HashMap<u32, f64> {
    let mut out = HashMap::new();
    let mut stack = vec![(m, 1.0f64)];
    while let Some((v, w)) = stack.pop() {
        if d.children(v).is_empty() {
            out.insert(d.leaf_ordinal(v).unwrap(), w);
        }
        for &(c, cw) in d.children(v) {
            stack.push((c, w * f64::from(cw)));
        }
    }
    out
}

fn random_tree(name: &str, parents: &[usize], weights: &[i8]) -> Dimension {
    let mut e = Dimension::new(name, DimensionKind::Generic).edit();
    let mut ids = vec![e.add("m0").unwrap()];
    for (i, p) in parents.iter().enumerate() {
        let id = e.add(&format!("m{}", i + 1)).unwrap();
        let w = [1.0, -1.0, 2.0][(weights[i % weights.len()].unsigned_abs() % 3) as usize];
        e.add_edge(ids[p % ids.len()], id, w).unwrap();
        ids.push(id);
    }
    e.commit().unwrap()
}

proptest! {
    /// Aggregate = Σ base × weight, for every cell, via `value` and via `view`.
    #[test]
    fn aggregate_equals_weighted_base_sum(
        pa in proptest::collection::vec(0usize..100, 1..25),
        pb in proptest::collection::vec(0usize..100, 1..25),
        pc in proptest::collection::vec(0usize..100, 1..10),
        w in proptest::collection::vec(any::<i8>(), 1..5),
        facts in proptest::collection::vec((any::<u16>(), any::<u16>(), any::<u16>(), 1i32..1000), 0..120),
    ) {
        let dims: Vec<Arc<Dimension>> = vec![
            Arc::new(random_tree("a", &pa, &w)),
            Arc::new(random_tree("b", &pb, &w)),
            Arc::new(random_tree("c", &pc, &w)),
        ];
        let mut cube = Cube::new("p", dims.clone());
        let mut base: HashMap<Vec<u32>, f64> = HashMap::new();
        for (x, y, z, v) in facts {
            let ords = [x as u32 % dims[0].n_leaves(), y as u32 % dims[1].n_leaves(), z as u32 % dims[2].n_leaves()];
            let addr: Vec<MemberId> = (0..3).map(|d| dims[d].leaf_member(ords[d]).unwrap()).collect();
            cube.set_base(&addr, Some(f64::from(v))).unwrap();
            base.insert(ords.to_vec(), f64::from(v));
        }
        let members = |d: usize| -> Vec<MemberId> { (0..dims[d].len() as u32).collect() };
        let weights: Vec<Vec<HashMap<u32, f64>>> =
            (0..3).map(|d| members(d).iter().map(|&m| naive_weights(&dims[d], m)).collect()).collect();
        let expect = |a: usize, b: usize, c: usize| -> Option<f64> {
            let mut hit = false;
            let mut s = 0.0;
            for (k, v) in &base {
                if let (Some(x), Some(y), Some(z)) = (weights[0][a].get(&k[0]), weights[1][b].get(&k[1]), weights[2][c].get(&k[2])) {
                    s += v * x * y * z;
                    hit = true;
                }
            }
            hit.then_some(s)
        };
        let (ma, mb, mc) = (members(0), members(1), members(2));
        for &a in &ma { for &b in &mb { for &c in &mc {
            let got = cube.value(&[a, b, c]).unwrap();
            let want = expect(a as usize, b as usize, c as usize);
            prop_assert_eq!(got.is_some(), want.is_some());
            if let (Some(g), Some(w)) = (got, want) { prop_assert!((g - w).abs() < 1e-6); }
        }}}
        for &c in &mc {
            let g = cube.view(0, &ma, 1, &mb, &[0, 0, c]).unwrap();
            for (ri, &a) in ma.iter().enumerate() { for (ci, &b) in mb.iter().enumerate() {
                let want = expect(a as usize, b as usize, c as usize);
                prop_assert_eq!(g.get(ri, ci).is_some(), want.is_some());
                if let (Some(x), Some(w)) = (g.get(ri, ci), want) { prop_assert!((x - w).abs() < 1e-6); }
            }}
        }
    }
}

/// M2 benchmark: 10 million cells × 20 dimensions.
/// `cargo test --release -p neolap-core -- --ignored --nocapture benchmark_10m`
#[test]
#[ignore]
fn benchmark_10m() {
    use std::time::Instant;
    const DIMS: usize = 20;
    const CELLS: usize = 10_000_000;
    // 1 root, 20 groups, 20 × 50 leaves (1,000 leaves + 21 aggregates).
    let dims: Vec<Arc<Dimension>> = (0..DIMS)
        .map(|i| {
            let mut e = Dimension::new(format!("d{i}"), DimensionKind::Generic).edit();
            let root = e.add("All").unwrap();
            for g in 0..20 {
                let gid = e.add(&format!("g{g}")).unwrap();
                e.add_edge(root, gid, 1.0).unwrap();
                for l in 0..50 {
                    let l = e.add(&format!("l{g}_{l}")).unwrap();
                    e.add_edge(gid, l, 1.0).unwrap();
                }
            }
            Arc::new(e.commit().unwrap())
        })
        .collect();
    let mut cube = Cube::new("big", dims.clone());
    let t = Instant::now();
    let mut x: u64 = 88172645463325252;
    let mut rnd = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut coords = [0u32; DIMS];
    let mut n = 0;
    while n < CELLS {
        for c in coords.iter_mut() {
            *c = (rnd() % 1000) as u32;
        }
        if cube
            .store
            .set(&coords, 1.0 + (rnd() % 100) as f64)
            .is_none()
        {
            n += 1;
        }
    }
    println!("load {CELLS} cells: {:?}", t.elapsed());
    let mut times = Vec::new();
    for g in 0..20usize {
        let mut addr: Vec<MemberId> = vec![dims[0].member("All").unwrap(); DIMS];
        addr[0] = dims[0].member(&format!("g{g}")).unwrap();
        addr[1] = dims[1].member(&format!("g{}", (g + 3) % 20)).unwrap();
        let t = Instant::now();
        let v = cube.value(&addr).unwrap();
        times.push(t.elapsed());
        let t2 = Instant::now();
        assert_eq!(cube.value(&addr).unwrap(), v);
        println!("cold {:?}, cached {:?}", times[g], t2.elapsed());
    }
    let grand: Vec<MemberId> = dims.iter().map(|d| d.member("All").unwrap()).collect();
    let t = Instant::now();
    let v = cube.value(&grand).unwrap();
    println!("grand total {v:?}: {:?}", t.elapsed());
    let rows: Vec<_> = (0..50)
        .map(|i| {
            dims[0]
                .member(&format!("l{}_{}", i / 50 % 20, i % 50))
                .unwrap()
        })
        .collect();
    let cols: Vec<_> = (0..20)
        .map(|g| dims[1].member(&format!("g{g}")).unwrap())
        .collect();
    let t = Instant::now();
    let g = cube.view(0, &rows, 1, &cols, &grand).unwrap();
    let el = t.elapsed();
    println!(
        "view 50x20: {el:?}, non-empty cells {}",
        g.cells.iter().flatten().count()
    );
    assert!(el.as_millis() < 500, "view too slow");
    assert!(
        times.iter().all(|t| t.as_millis() < 100),
        "cold cell too slow"
    );
}
