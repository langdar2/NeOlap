//! Generator for large synthetic models (plan chapter 11): up to 20 dimensions, up to 500,000
//! members, unbalanced trees up to depth 50, members with two parents, some negative weights.
//!
//! Deterministic: the same arguments always give the same model, so measurements are comparable.

use std::sync::Arc;

use crate::MemberId;
use crate::cube::Cube;
use crate::dimension::{Dimension, DimensionKind};

/// Small reproducible random numbers (xorshift64*); no dependency.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

/// Shape of one generated dimension.
#[derive(Clone, Copy)]
pub struct Shape {
    /// Members in total (base members and aggregates).
    pub members: usize,
    /// Depth of the deepest branch.
    pub depth: usize,
    /// Percent of base members that get a second parent.
    pub second_parent_percent: u64,
    /// Percent of base members with weight -1 (only those with a single parent).
    pub negative_percent: u64,
}

/// A spine of `depth` aggregates makes the deepest branch. Further aggregates hang at random
/// places of the spine and of each other, base members below random aggregates, so subtrees
/// differ widely in size and depth.
pub fn dimension(name: &str, shape: Shape, seed: u64) -> Dimension {
    let mut rng = Rng::new(seed);
    let mut e = Dimension::new(name, DimensionKind::Generic).edit();
    let root = e.add("All").unwrap();
    let mut inner: Vec<MemberId> = vec![root];
    for level in 1..shape.depth.max(1) {
        let m = e.add(&format!("s{level}")).unwrap();
        e.add_edge(*inner.last().unwrap(), m, 1.0).unwrap();
        inner.push(m);
    }
    let n_aggregates = (shape.members / 12).max(1);
    for i in 0..n_aggregates {
        let p = inner[rng.below(inner.len())];
        let m = e.add(&format!("a{i}")).unwrap();
        e.add_edge(p, m, 1.0).unwrap();
        inner.push(m);
    }
    let base = shape.members.saturating_sub(inner.len());
    for i in 0..base {
        let p = inner[rng.below(inner.len())];
        let m = e.add(&format!("b{i}")).unwrap();
        if rng.chance(shape.second_parent_percent) {
            e.add_edge(p, m, 1.0).unwrap();
            let q = inner[rng.below(inner.len())];
            if q != p {
                e.add_edge(q, m, 1.0).unwrap();
            }
        } else {
            let w = if rng.chance(shape.negative_percent) {
                -1.0
            } else {
                1.0
            };
            e.add_edge(p, m, w).unwrap();
        }
    }
    e.commit().unwrap()
}

/// Puts `cells` random base cells into the cube (uniform over each dimension's leaves).
pub fn fill(cube: &mut Cube, cells: usize, seed: u64) {
    let leaves: Vec<usize> = cube
        .dimensions()
        .iter()
        .map(|d| d.n_leaves() as usize)
        .collect();
    let mut rng = Rng::new(seed);
    let mut coords = vec![0u32; leaves.len()];
    let mut n = 0;
    while n < cells {
        for (c, &l) in coords.iter_mut().zip(&leaves) {
            *c = rng.below(l) as u32;
        }
        if cube
            .store_mut()
            .set(&coords, 1.0 + rng.below(100) as f64)
            .is_none()
        {
            n += 1;
        }
    }
}

#[test]
fn generated_dimension_has_the_requested_shape() {
    let shape = Shape {
        members: 20_000,
        depth: 50,
        second_parent_percent: 10,
        negative_percent: 3,
    };
    let d = dimension("x", shape, 1);
    assert_eq!(d.len(), 20_000);
    assert!(d.n_leaves() > 15_000);
    let again = dimension("x", shape, 1);
    assert_eq!(again.n_leaves(), d.n_leaves(), "deterministic");
}

/// M11: the chapter 1 targets on a realistic model. Two dimensions of 500,000 members and
/// depth 50, four of 20,000, the rest small; 20 million cells.
/// `cargo test --release -p neolap-core --lib -- --ignored --nocapture synth_targets`
#[test]
#[ignore]
fn synth_targets() {
    use std::time::Instant;
    let cells: usize = std::env::var("CELLS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000_000);
    let big = Shape {
        members: 500_000,
        depth: 50,
        second_parent_percent: 5,
        negative_percent: 1,
    };
    let mid = Shape {
        members: 20_000,
        depth: 30,
        second_parent_percent: 5,
        negative_percent: 1,
    };
    let small = Shape {
        members: 300,
        depth: 6,
        second_parent_percent: 2,
        negative_percent: 0,
    };
    let t = Instant::now();
    let mut dims = Vec::new();
    for i in 0..20u64 {
        let shape = match i {
            0 | 1 => big,
            2..=5 => mid,
            _ => small,
        };
        dims.push(Arc::new(dimension(&format!("d{i}"), shape, 100 + i)));
    }
    println!("build 20 dimensions: {:?}", t.elapsed());
    let mut cube = Cube::new("synth", dims.clone());
    let t = Instant::now();
    fill(&mut cube, cells, 7);
    println!("fill {cells} cells: {:?}", t.elapsed());
    println!("memory: {:?}", cube.store().memory_stats());

    let all: Vec<MemberId> = dims.iter().map(|d| d.member("All").unwrap()).collect();
    let mut rng = Rng::new(42);
    let mut cold = Vec::new();
    let mut cached = Vec::new();
    for _ in 0..20 {
        let mut addr = all.clone();
        for d in 0..4 {
            let name = format!("a{}", rng.below(dims[d].len() / 12));
            addr[d] = dims[d].member(&name).unwrap();
        }
        let t = Instant::now();
        let v = cube.value(&addr).unwrap();
        cold.push(t.elapsed());
        let t = Instant::now();
        assert_eq!(cube.value(&addr).unwrap(), v);
        cached.push(t.elapsed());
    }
    cold.sort();
    cached.sort();
    println!("aggregate cold: median {:?}, max {:?}", cold[10], cold[19]);
    println!(
        "aggregate cached: median {:?}, max {:?}",
        cached[10], cached[19]
    );

    // worst case: large member sets (the root and the upper spine) on several dimensions
    for (label, names) in [
        ("root x4", ["All", "All", "All", "All"]),
        ("s1 x4", ["s1", "s1", "s1", "s1"]),
        ("s10/s20", ["s10", "s20", "s5", "All"]),
    ] {
        let mut addr = all.clone();
        for (d, n) in names.iter().enumerate() {
            addr[d] = dims[d].member(n).unwrap();
        }
        let t = Instant::now();
        let v = cube.value(&addr).unwrap();
        let first = t.elapsed();
        let t = Instant::now();
        cube.value(&addr).unwrap();
        println!(
            "{label}: cold {first:?}, cached {:?}, value {v:?}",
            t.elapsed()
        );
    }
    let rows: Vec<MemberId> = (0..50)
        .map(|i| dims[0].member(&format!("a{i}")).unwrap())
        .collect();
    let cols: Vec<MemberId> = (0..20)
        .map(|i| dims[1].member(&format!("a{i}")).unwrap())
        .collect();
    let t = Instant::now();
    let g = cube.view(0, &rows, 1, &cols, &all).unwrap();
    println!(
        "pivot 50x20: {:?}, non-empty {}",
        t.elapsed(),
        g.cells.iter().flatten().count()
    );
}
