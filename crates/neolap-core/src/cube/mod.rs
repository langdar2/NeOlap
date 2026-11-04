//! Cubes: base-cell storage plus aggregation at query time.
//!
//! Only base cells are stored. An aggregate cell is answered from the `LeafSet`s of its
//! members (see `docs/Implementierungsplan.md`, chapter 4.2): the most selective dimension
//! provides the row set, the other dimensions filter it.

mod calc;
mod input;
mod store;
mod write;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use rayon::prelude::*;

use crate::dimension::{Dimension, LeafSet};
use crate::rules::RuleSet;
use crate::{Error, MemberId, Result};

pub use calc::{CellError, CellValue, CubeProvider};
pub use input::Input;
pub use store::{FactStore, MemoryStats};
pub use write::{
    AreaInfo, BulkMode, BulkOutcome, BulkWrite, CellChange, CellVersion, OpRecord, OpSink,
    UNDO_DEPTH, WriteContext, WritePermission, WriteResult,
};

/// Sets with more base members than this are not summed up for the row estimate.
const MAX_ESTIMATE_LEAVES: usize = 2048;
/// Rows per parallel scan task.
const SCAN_CHUNK: usize = 64 * 1024;
/// Upper bound of cached aggregate cells.
const CACHE_CAPACITY: usize = 200_000;

pub struct Cube {
    name: String,
    dims: Vec<Arc<Dimension>>,
    store: FactStore,
    /// Incremented by every write; clients and the cache refer to it.
    generation: u64,
    cache: Mutex<HashMap<Vec<MemberId>, Option<f64>>>,
    history: write::History,
    rules: RuleSet,
    /// Receives every operation before it is applied (write-through persistence).
    sink: Option<Box<dyn OpSink>>,
}

/// Result grid of [`Cube::view`], row-major; `None` = empty cell.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub n_rows: usize,
    pub n_cols: usize,
    pub cells: Vec<Option<f64>>,
    /// Cells that evaluate to an error (cell index = `row * n_cols + col`); their `cells` entry is `None`.
    pub errors: HashMap<usize, CellError>,
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
            history: write::History::default(),
            rules: RuleSet::default(),
            sink: None,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Sets the generation without any other effect (after loading).
    pub fn restore_generation(&mut self, generation: u64) {
        self.generation = generation;
    }

    /// Installs the sink that persists operations; see [`OpSink`].
    pub fn set_sink(&mut self, sink: Option<Box<dyn OpSink>>) {
        self.sink = sink;
    }

    /// All filled cells keyed by member ids (stable across dimension changes).
    pub fn export_cells(&self) -> Vec<(Vec<MemberId>, f64)> {
        self.store
            .iter()
            .map(|(coords, v)| (self.coords_to_members(&coords), v))
            .collect()
    }

    pub(crate) fn coords_to_members(&self, coords: &[u32]) -> Vec<MemberId> {
        self.dims
            .iter()
            .zip(coords)
            .map(|(d, &o)| d.leaf_member(o).expect("stored ordinal"))
            .collect()
    }

    /// Loads cells (member ids of base members) without recording operations. Replaces the content
    /// of the cube's fact store; the generation is set to `generation`.
    pub fn load_cells(
        &mut self,
        cells: impl IntoIterator<Item = (Vec<MemberId>, f64)>,
        generation: u64,
    ) -> Result<()> {
        let mut store = FactStore::new(self.dims.len());
        for (addr, v) in cells {
            let coords = self.base_coords(&addr)?;
            store.set(&coords, v);
        }
        self.store = store;
        self.generation = generation;
        self.cache.lock().clear();
        Ok(())
    }

    /// Exchanges the dimension at `index` for its edited successor. Cells of removed members are
    /// dropped (returned for the journal); cells of members that became aggregates are dropped too.
    /// Ordinals are recalculated, the operation log is discarded (its coordinates are void).
    pub fn replace_dimension(
        &mut self,
        index: usize,
        new: Arc<Dimension>,
    ) -> Result<Vec<(Vec<MemberId>, f64)>> {
        if index >= self.dims.len() {
            return Err(Error::InvalidOperation(format!(
                "cube '{}' has no dimension {index}",
                self.name
            )));
        }
        let mut kept = Vec::with_capacity(self.store.len());
        let mut dropped = Vec::new();
        for (addr, v) in self.export_cells() {
            let m = addr[index];
            if new.is_leaf(m) {
                kept.push((addr, v));
            } else {
                dropped.push((addr, v));
            }
        }
        self.dims[index] = new;
        let generation = self.generation + 1;
        self.load_cells(kept, generation)?;
        self.history = write::History::floor_at(generation);
        // rules refer to member ids and dimension names: resolve them again
        let text = self.rules.text.clone();
        self.rules = RuleSet::build(&text, &self.dims).unwrap_or_default();
        Ok(dropped)
    }

    pub fn dimensions(&self) -> &[Arc<Dimension>] {
        &self.dims
    }

    pub fn store(&self) -> &FactStore {
        &self.store
    }

    /// Direct write access for the synthetic benchmark models (no operation log, no cache reset).
    #[cfg(test)]
    pub(crate) fn store_mut(&mut self) -> &mut FactStore {
        &mut self.store
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Number of filled base cells.
    pub fn n_cells(&self) -> usize {
        self.store.len()
    }

    pub(crate) fn check_addr(&self, addr: &[MemberId]) -> Result<()> {
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

    pub(crate) fn base_coords(&self, addr: &[MemberId]) -> Result<Vec<u32>> {
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

    /// Number of aggregate results currently cached (base cells are never cached: they are one lookup).
    pub fn cached_cells(&self) -> usize {
        self.cache.lock().len()
    }

    /// Value of a cell (base or aggregate); `None` if no base cell contributes.
    pub fn value(&self, addr: &[MemberId]) -> Result<Option<f64>> {
        self.check_addr(addr)?;
        // A base cell is stored, so it is one hash lookup. Without this, reading a cell cost a scan
        // over all rows (and a cache entry), and an export of 120,000 cells took almost three minutes.
        let leaves: Option<Vec<u32>> = self
            .dims
            .iter()
            .zip(addr)
            .map(|(d, &m)| d.leaf_ordinal(m))
            .collect();
        if let Some(coords) = leaves {
            return Ok(self.store.get(&coords));
        }
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
        // Active dimensions (not "all"), most selective first, so the scan can stop early per row.
        let mut active: Vec<(usize, usize)> = Vec::new();
        for (d, set) in sets.iter().enumerate() {
            if is_all(set, self.dims[d].n_leaves()) {
                continue;
            }
            let est = if set.len() > MAX_ESTIMATE_LEAVES {
                total
            } else {
                set.iter().map(|o| self.store.count_of_leaf(d, o)).sum()
            };
            if est == 0 {
                return None;
            }
            active.push((d, est));
        }
        active.sort_by_key(|a| a.1);
        let active: Vec<usize> = active.into_iter().map(|a| a.0).collect();
        let weighted: Vec<usize> = (0..sets.len()).filter(|&d| sets[d].has_weights()).collect();

        let store = &self.store;
        let (sum, hit) = (0..store.n_rows())
            .step_by(SCAN_CHUNK)
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|start| {
                let (mut sum, mut hit) = (0.0, false);
                for row in start..(start + SCAN_CHUNK).min(store.n_rows()) {
                    if !store.is_live(row)
                        || !active.iter().all(|&d| sets[d].contains(store.col(d)[row]))
                    {
                        continue;
                    }
                    let mut v = store.value(row);
                    for &d in &weighted {
                        v *= f64::from(sets[d].weight(store.col(d)[row]));
                    }
                    sum += v;
                    hit = true;
                }
                (sum, hit)
            })
            .reduce(|| (0.0, false), |a, b| (a.0 + b.0, a.1 | b.1));
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
        self.view_with(row_dim, rows, col_dim, cols, fixed, None)
    }

    /// Like [`view`](Self::view); cells influenced by rules are evaluated with them.
    pub fn view_with(
        &self,
        row_dim: usize,
        rows: &[MemberId],
        col_dim: usize,
        cols: &[MemberId],
        fixed: &[MemberId],
        db: Option<&dyn CubeProvider>,
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
        let mut cells: Vec<Option<f64>> = sums
            .into_iter()
            .zip(hits)
            .map(|(s, h)| h.then_some(s))
            .collect();
        let mut errors = HashMap::new();
        if !self.rules.is_empty() {
            let mut addr = fixed.to_vec();
            for (ri, &r) in rows.iter().enumerate() {
                for (ci, &c) in cols.iter().enumerate() {
                    addr[row_dim] = r;
                    addr[col_dim] = c;
                    if !self.rule_touches(&addr) {
                        continue;
                    }
                    let i = ri * n_cols + ci;
                    cells[i] = None;
                    match self.cell(&addr, db)? {
                        CellValue::Num(v) => cells[i] = Some(v),
                        CellValue::Empty => {}
                        CellValue::Text(_) => {
                            errors.insert(i, CellError::Err);
                        }
                        CellValue::Error(e) => {
                            errors.insert(i, e);
                        }
                    }
                }
            }
        }
        Ok(Grid {
            n_rows: rows.len(),
            n_cols,
            cells,
            errors,
        })
    }
}

#[cfg(test)]
mod rule_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod write_tests;
