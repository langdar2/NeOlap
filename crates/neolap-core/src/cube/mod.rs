//! Cubes: base-cell storage plus aggregation at query time.
//!
//! Only base cells are stored. An aggregate cell is answered from the `LeafSet`s of its
//! members (see `docs/Implementierungsplan.md`, chapter 4.2): the most selective dimension
//! provides the row set, the other dimensions filter it.

mod store;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use rayon::prelude::*;
use roaring::RoaringBitmap;

use crate::dimension::{Dimension, LeafSet};
use crate::{Error, MemberId, Result};

pub use store::FactStore;

/// Sets with more base members than this are not turned into bitmap unions.
const MAX_UNION_LEAVES: usize = 2048;
/// Upper bound of cached aggregate cells.
const CACHE_CAPACITY: usize = 200_000;

pub struct Cube {
    name: String,
    dims: Vec<Arc<Dimension>>,
    store: FactStore,
    /// Incremented by every write; clients and the cache refer to it.
    generation: u64,
    cache: Mutex<HashMap<Vec<MemberId>, Option<f64>>>,
}

/// Result grid of [`Cube::view`], row-major; `None` = empty cell.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub n_rows: usize,
    pub n_cols: usize,
    pub cells: Vec<Option<f64>>,
}

impl Grid {
    pub fn get(&self, row: usize, col: usize) -> Option<f64> {
        self.cells[row * self.n_cols + col]
    }

    /// Indices of rows / columns containing at least one value (zero suppression).
    pub fn non_empty_rows(&self) -> Vec<usize> {
        (0..self.n_rows)
            .filter(|&r| (0..self.n_cols).any(|c| self.get(r, c).is_some()))
            .collect()
    }

    pub fn non_empty_cols(&self) -> Vec<usize> {
        (0..self.n_cols)
            .filter(|&c| (0..self.n_rows).any(|r| self.get(r, c).is_some()))
            .collect()
    }
}

fn is_all(set: &LeafSet, n_leaves: u32) -> bool {
    !set.has_weights() && set.as_range() == Some(0..n_leaves)
}

impl Cube {
    pub fn new(name: impl Into<String>, dims: Vec<Arc<Dimension>>) -> Cube {
        let n = dims.len();
        Cube {
            name: name.into(),
            dims,
            store: FactStore::new(n),
            generation: 0,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn dimensions(&self) -> &[Arc<Dimension>] {
        &self.dims
    }

    pub fn store(&self) -> &FactStore {
        &self.store
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Number of filled base cells.
    pub fn n_cells(&self) -> usize {
        self.store.len()
    }

    fn check_addr(&self, addr: &[MemberId]) -> Result<()> {
        if addr.len() != self.dims.len() {
            return Err(Error::InvalidOperation(format!(
                "cube '{}' has {} dimensions, address has {}",
                self.name,
                self.dims.len(),
                addr.len()
            )));
        }
        for (d, &m) in self.dims.iter().zip(addr) {
            if d.member_name(m).is_none() {
                return Err(Error::NotFound {
                    kind: "member id",
                    name: format!("{m} in '{}'", d.name()),
                });
            }
        }
        Ok(())
    }

    fn base_coords(&self, addr: &[MemberId]) -> Result<Vec<u32>> {
        self.check_addr(addr)?;
        self.dims
            .iter()
            .zip(addr)
            .map(|(d, &m)| {
                d.leaf_ordinal(m).ok_or_else(|| {
                    Error::InvalidOperation(format!(
                        "'{}' in '{}' is an aggregate, not a base member",
                        d.member_name(m).unwrap_or("?"),
                        d.name()
                    ))
                })
            })
            .collect()
    }

    /// Resolves member names (one per dimension, case-insensitive) to an address.
    pub fn address(&self, names: &[&str]) -> Result<Vec<MemberId>> {
        if names.len() != self.dims.len() {
            return Err(Error::InvalidOperation(format!(
                "expected {} members, got {}",
                self.dims.len(),
                names.len()
            )));
        }
        self.dims
            .iter()
            .zip(names)
            .map(|(d, n)| d.member(n))
            .collect()
    }

    fn touch(&mut self) {
        self.generation += 1;
        self.cache.lock().clear();
    }

    /// Writes a base cell. Writing `None` deletes it. Returns the previous value.
    pub fn set_base(&mut self, addr: &[MemberId], value: Option<f64>) -> Result<Option<f64>> {
        let coords = self.base_coords(addr)?;
        let old = match value {
            Some(v) => self.store.set(&coords, v),
            None => self.store.remove(&coords),
        };
        if old != value {
            self.touch();
        }
        Ok(old)
    }

    /// Value of a cell (base or aggregate); `None` if no base cell contributes.
    pub fn value(&self, addr: &[MemberId]) -> Result<Option<f64>> {
        self.check_addr(addr)?;
        if let Some(hit) = self.cache.lock().get(addr) {
            return Ok(*hit);
        }
        let sets: Vec<Arc<LeafSet>> = self
            .dims
            .iter()
            .zip(addr)
            .map(|(d, &m)| d.leaf_set(m))
            .collect();
        let v = self.aggregate(&sets);
        let mut cache = self.cache.lock();
        if cache.len() >= CACHE_CAPACITY {
            cache.clear();
        }
        cache.insert(addr.to_vec(), v);
        Ok(v)
    }

    fn aggregate(&self, sets: &[Arc<LeafSet>]) -> Option<f64> {
        if sets.iter().any(|s| s.is_empty()) {
            return None;
        }
        let total = self.store.len();
        // Active dimensions (not "all") with an estimate of the matching rows.
        let mut active: Vec<(usize, usize)> = Vec::new();
        for (d, set) in sets.iter().enumerate() {
            if is_all(set, self.dims[d].n_leaves()) {
                continue;
            }
            let est = if set.len() > MAX_UNION_LEAVES {
                total
            } else {
                set.iter()
                    .map(|o| {
                        self.store
                            .rows_of_leaf(d, o)
                            .map_or(0, |b| b.len() as usize)
                    })
                    .sum()
            };
            if est == 0 {
                return None;
            }
            active.push((d, est));
        }
        active.sort_by_key(|a| a.1);

        let weighted: Vec<usize> = (0..sets.len()).filter(|&d| sets[d].has_weights()).collect();
        let mut sum = 0.0;
        let mut hit = false;
        let mut visit = |row: usize| {
            for &(d, _) in active.iter().skip(1) {
                if !sets[d].contains(self.store.col(d)[row]) {
                    return;
                }
            }
            let mut v = self.store.value(row);
            for &d in &weighted {
                v *= f64::from(sets[d].weight(self.store.col(d)[row]));
            }
            sum += v;
            hit = true;
        };
        match active.first() {
            None => (0..self.store.n_rows())
                .filter(|&r| self.store.is_live(r))
                .for_each(&mut visit),
            Some(&(d, _)) if sets[d].len() <= MAX_UNION_LEAVES => {
                let mut rows = RoaringBitmap::new();
                for o in sets[d].iter() {
                    if let Some(b) = self.store.rows_of_leaf(d, o) {
                        rows |= b;
                    }
                }
                rows.iter().for_each(|r| visit(r as usize));
            }
            Some(&(d, _)) => {
                let col = self.store.col(d);
                (0..self.store.n_rows())
                    .filter(|&r| self.store.is_live(r) && sets[d].contains(col[r]))
                    .for_each(&mut visit);
            }
        }
        hit.then_some(sum)
    }

    /// Pivot view: `rows × cols` cells over two axes; `fixed` holds one member per dimension, the
    /// entries of the two axis dimensions are ignored. All facts are visited once and assigned to the
    /// visible axis members (grouping path), in parallel.
    pub fn view(
        &self,
        row_dim: usize,
        rows: &[MemberId],
        col_dim: usize,
        cols: &[MemberId],
        fixed: &[MemberId],
    ) -> Result<Grid> {
        if row_dim == col_dim || row_dim >= self.dims.len() || col_dim >= self.dims.len() {
            return Err(Error::InvalidOperation("invalid axis dimensions".into()));
        }
        self.check_addr(fixed)?;
        for (dim, members) in [(row_dim, rows), (col_dim, cols)] {
            for &m in members {
                if self.dims[dim].member_name(m).is_none() {
                    return Err(Error::NotFound {
                        kind: "member id",
                        name: m.to_string(),
                    });
                }
            }
        }
        // Base ordinal → (axis index, weight).
        let axis_table = |dim: usize, members: &[MemberId]| -> Vec<Vec<(u32, f32)>> {
            let mut t = vec![Vec::new(); self.dims[dim].n_leaves() as usize];
            for (i, &m) in members.iter().enumerate() {
                let set = self.dims[dim].leaf_set(m);
                for o in set.iter() {
                    t[o as usize].push((i as u32, set.weight(o)));
                }
            }
            t
        };
        let row_t = axis_table(row_dim, rows);
        let col_t = axis_table(col_dim, cols);
        let filters: Vec<(usize, Arc<LeafSet>)> = (0..self.dims.len())
            .filter(|&d| d != row_dim && d != col_dim)
            .map(|d| (d, self.dims[d].leaf_set(fixed[d])))
            .filter(|(d, s)| !is_all(s, self.dims[*d].n_leaves()))
            .collect();
        let weighted: Vec<&(usize, Arc<LeafSet>)> =
            filters.iter().filter(|f| f.1.has_weights()).collect();

        let size = rows.len() * cols.len();
        let n_cols = cols.len();
        let store = &self.store;
        let chunk = 64 * 1024;
        let (sums, hits) = (0..store.n_rows())
            .step_by(chunk)
            .collect::<Vec<_>>()
            .into_par_iter()
            .fold(
                || (vec![0.0f64; size], vec![false; size]),
                |(mut sums, mut hits), start| {
                    for row in start..(start + chunk).min(store.n_rows()) {
                        if !store.is_live(row) {
                            continue;
                        }
                        let rt = &row_t[store.col(row_dim)[row] as usize];
                        let ct = &col_t[store.col(col_dim)[row] as usize];
                        if rt.is_empty() || ct.is_empty() {
                            continue;
                        }
                        if !filters.iter().all(|(d, s)| s.contains(store.col(*d)[row])) {
                            continue;
                        }
                        let mut v = store.value(row);
                        for (d, s) in &weighted {
                            v *= f64::from(s.weight(store.col(*d)[row]));
                        }
                        for &(ri, rw) in rt {
                            for &(ci, cw) in ct {
                                let i = ri as usize * n_cols + ci as usize;
                                sums[i] += v * f64::from(rw) * f64::from(cw);
                                hits[i] = true;
                            }
                        }
                    }
                    (sums, hits)
                },
            )
            .reduce(
                || (vec![0.0; size], vec![false; size]),
                |(mut a, mut ah), (b, bh)| {
                    for i in 0..size {
                        a[i] += b[i];
                        ah[i] |= bh[i];
                    }
                    (a, ah)
                },
            );
        let cells = sums
            .into_iter()
            .zip(hits)
            .map(|(s, h)| h.then_some(s))
            .collect();
        Ok(Grid {
            n_rows: rows.len(),
            n_cols,
            cells,
        })
    }
}

#[cfg(test)]
mod tests;
