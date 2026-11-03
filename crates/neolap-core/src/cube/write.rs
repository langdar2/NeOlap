//! Write-back: splash modes, protection rules, operation log, conflict detection and undo.
//!
//! One write is one operation: it changes any number of base cells, raises the cube generation
//! once and is recorded (old and new value per cell) in an in-memory log. The log feeds conflict
//! detection against a client's generation, undo and cell versions. M5 persists it as journal.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use rayon::prelude::*;

use super::input::Input;
use super::{Cube, SCAN_CHUNK, is_all};
use crate::dimension::LeafSet;
use crate::{Error, MemberId, Result};

/// Operations per user that can be undone.
pub const UNDO_DEPTH: usize = 50;
/// Cell changes kept in the in-memory log; older operations drop out.
const MAX_LOG_CELLS: usize = 1_000_000;
const NEAR_ZERO: f64 = 1e-9;

/// Checks whether a base cell (leaf ordinal per dimension) may be written.
pub type WritePermission = Arc<dyn Fn(&[u32]) -> bool + Send + Sync>;

/// Who writes, against which view, and under which limits.
#[derive(Clone)]
pub struct WriteContext {
    pub user: String,
    /// Generation of the client's view; a change of the affected area since then is a conflict.
    pub expected_generation: Option<u64>,
    /// Upper bound of cells created by an equal splash (and by a fallback to it).
    pub max_equal_cells: usize,
    /// Rounding of written values; the rounding difference goes to the largest cell so that
    /// the target sum is exact. `None` = no rounding.
    pub decimals: Option<u32>,
    /// Write permission per base cell; `None` = everything is allowed.
    pub can_write: Option<WritePermission>,
}

impl WriteContext {
    pub fn new(user: impl Into<String>) -> Self {
        WriteContext {
            user: user.into(),
            expected_generation: None,
            max_equal_cells: 1_000_000,
            decimals: None,
            can_write: None,
        }
    }

    pub fn expecting(mut self, generation: u64) -> Self {
        self.expected_generation = Some(generation);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WriteResult {
    /// `None` if nothing changed (no operation was recorded).
    pub op_id: Option<u64>,
    pub generation: u64,
    pub cells_changed: usize,
}

/// Size of the base area behind an address, for the "n cells affected" preview in the UI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AreaInfo {
    pub base_cells: u128,
    pub filled_cells: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CellVersion {
    pub generation: u64,
    pub user: String,
}

struct Change {
    coords: Vec<u32>,
    old: Option<f64>,
    new: Option<f64>,
}

/// One changed cell of an operation, addressed by member ids.
#[derive(Debug, Clone, PartialEq)]
pub struct CellChange {
    pub addr: Vec<MemberId>,
    pub old: Option<f64>,
    pub new: Option<f64>,
}

/// An operation as handed to the [`OpSink`] before it is applied.
#[derive(Debug, Clone, PartialEq)]
pub struct OpRecord {
    pub id: u64,
    pub generation: u64,
    pub user: String,
    /// `set`, `splash` or `undo`
    pub kind: &'static str,
    pub changes: Vec<CellChange>,
}

/// Persists operations. `record` is called before the operation is applied to memory; if it fails,
/// the operation is rejected and nothing changes (one operation = one transaction).
pub trait OpSink: Send + Sync {
    fn record(&mut self, cube: &str, op: &OpRecord) -> Result<()>;
}

pub(super) struct Op {
    id: u64,
    generation: u64,
    user: String,
    /// `len() * n_dims` ordinals.
    coords: Vec<u32>,
    /// NaN = cell did not exist / was deleted.
    old: Vec<f64>,
    new: Vec<f64>,
}

impl Op {
    fn len(&self) -> usize {
        self.old.len()
    }

    fn coords_of(&self, i: usize, n_dims: usize) -> &[u32] {
        &self.coords[i * n_dims..(i + 1) * n_dims]
    }
}

#[derive(Default)]
pub(super) struct History {
    ops: VecDeque<Op>,
    cells: usize,
    /// Generation of the newest operation that dropped out of the log.
    floor: u64,
    next_id: u64,
    undo: HashMap<String, Vec<u64>>,
}

impl History {
    /// An empty log that rejects views older than `generation`.
    pub(super) fn floor_at(generation: u64) -> History {
        History {
            floor: generation,
            ..History::default()
        }
    }

    /// User of the newest operation after position `pos` that touched `coords`.
    fn last_toucher(&self, pos: usize, coords: &[u32], n_dims: usize) -> Option<&str> {
        self.ops
            .iter()
            .skip(pos + 1)
            .rev()
            .find(|op| (0..op.len()).any(|i| op.coords_of(i, n_dims) == coords))
            .map(|op| op.user.as_str())
    }
}

fn enc(v: Option<f64>) -> f64 {
    v.unwrap_or(f64::NAN)
}

fn dec(v: f64) -> Option<f64> {
    (!v.is_nan()).then_some(v)
}

fn round_to(v: f64, decimals: Option<u32>) -> f64 {
    match decimals {
        Some(d) => {
            let f = 10f64.powi(d as i32);
            (v * f).round() / f
        }
        None => v,
    }
}

fn area_len(sets: &[Arc<LeafSet>]) -> u128 {
    sets.iter().map(|s| s.len() as u128).product()
}

/// All base combinations of the area (ordinals per dimension).
fn combos(sets: &[Arc<LeafSet>]) -> Vec<Vec<u32>> {
    let lists: Vec<Vec<u32>> = sets.iter().map(|s| s.iter().collect()).collect();
    let mut out = Vec::with_capacity(lists.iter().map(Vec::len).product());
    let mut idx = vec![0usize; lists.len()];
    loop {
        out.push(idx.iter().enumerate().map(|(d, &i)| lists[d][i]).collect());
        let mut d = lists.len();
        loop {
            if d == 0 {
                return out;
            }
            d -= 1;
            idx[d] += 1;
            if idx[d] < lists[d].len() {
                break;
            }
            idx[d] = 0;
        }
    }
}

/// Turns the new cell values of the area into changes: books the rounding difference, deletes
/// existing cells that are not part of the result, skips unchanged cells, 0 = delete.
fn settle(
    decimals: Option<u32>,
    target: Option<f64>,
    mut news: Vec<(Vec<u32>, f64)>,
    existing: Vec<(Vec<u32>, f64)>,
) -> Vec<Change> {
    if let (Some(t), false) = (target, news.is_empty()) {
        let sum: f64 = news.iter().map(|n| n.1).sum();
        let residual = round_to(t - sum, decimals);
        if residual != 0.0 {
            let mut best = 0;
            for (i, n) in news.iter().enumerate() {
                if n.1.abs() > news[best].1.abs() {
                    best = i;
                }
            }
            news[best].1 += residual;
        }
    }
    let mut old: HashMap<Vec<u32>, f64> = existing.into_iter().collect();
    let mut changes = Vec::new();
    for (coords, v) in news {
        let o = old.remove(&coords);
        let n = (v != 0.0).then_some(v);
        if o != n {
            changes.push(Change {
                coords,
                old: o,
                new: n,
            });
        }
    }
    changes.extend(old.into_iter().map(|(coords, v)| Change {
        coords,
        old: Some(v),
        new: None,
    }));
    changes.sort_by(|a, b| a.coords.cmp(&b.coords));
    changes
}

impl Cube {
    /// Sets the base-member sets of an address; rejects areas that cannot be distributed uniquely.
    fn area_sets(&self, addr: &[MemberId]) -> Result<Vec<Arc<LeafSet>>> {
        self.check_addr(addr)?;
        let mut sets = Vec::with_capacity(addr.len());
        for (d, &m) in self.dims.iter().zip(addr) {
            let set = d.leaf_set(m);
            if set.has_weights() {
                return Err(Error::InvalidOperation(format!(
                    "'{}' in '{}' contains members with weight other than 1; distributing a value onto it is not unique",
                    d.member_name(m).unwrap_or("?"),
                    d.name()
                )));
            }
            sets.push(set);
        }
        Ok(sets)
    }

    fn rows_in(&self, sets: &[Arc<LeafSet>]) -> Vec<u32> {
        let active: Vec<usize> = (0..sets.len())
            .filter(|&d| !is_all(&sets[d], self.dims[d].n_leaves()))
            .collect();
        let store = &self.store;
        (0..store.n_rows())
            .step_by(SCAN_CHUNK)
            .collect::<Vec<_>>()
            .into_par_iter()
            .flat_map_iter(|start| {
                (start..(start + SCAN_CHUNK).min(store.n_rows()))
                    .filter(|&r| {
                        store.is_live(r)
                            && active.iter().all(|&d| sets[d].contains(store.col(d)[r]))
                    })
                    .map(|r| r as u32)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn existing_cells(&self, sets: &[Arc<LeafSet>]) -> Vec<(Vec<u32>, f64)> {
        if sets.iter().all(|s| s.len() == 1) {
            let coords: Vec<u32> = sets.iter().filter_map(|s| s.iter().next()).collect();
            return self
                .store
                .get(&coords)
                .map(|v| (coords, v))
                .into_iter()
                .collect();
        }
        self.rows_in(sets)
            .into_iter()
            .map(|r| {
                let r = r as usize;
                (
                    (0..self.dims.len()).map(|d| self.store.col(d)[r]).collect(),
                    self.store.value(r),
                )
            })
            .collect()
    }

    fn cell_name(&self, coords: &[u32]) -> String {
        let names: Vec<&str> = self
            .dims
            .iter()
            .zip(coords)
            .map(|(d, &o)| {
                d.leaf_member(o)
                    .and_then(|m| d.member_name(m))
                    .unwrap_or("?")
            })
            .collect();
        names.join(", ")
    }

    /// Number of base combinations and filled cells behind an address.
    pub fn area_info(&self, addr: &[MemberId]) -> Result<AreaInfo> {
        let sets = self.area_sets(addr)?;
        Ok(AreaInfo {
            base_cells: area_len(&sets),
            filled_cells: self.existing_cells(&sets).len(),
        })
    }

    /// Generation and user of the last change of a base cell (within the retained log).
    pub fn cell_version(&self, addr: &[MemberId]) -> Result<Option<CellVersion>> {
        let coords = self.base_coords(addr)?;
        let n = self.dims.len();
        Ok(self
            .history
            .ops
            .iter()
            .rev()
            .find(|op| (0..op.len()).any(|i| op.coords_of(i, n) == coords.as_slice()))
            .map(|op| CellVersion {
                generation: op.generation,
                user: op.user.clone(),
            }))
    }

    /// Writes a base cell directly (value `None` or 0 deletes). Returns the previous value.
    pub fn set_base(&mut self, addr: &[MemberId], value: Option<f64>) -> Result<Option<f64>> {
        let coords = self.base_coords(addr)?;
        let old = self.store.get(&coords);
        let new = value.filter(|v| *v != 0.0);
        if old != new {
            self.commit("system", "set", true, vec![Change { coords, old, new }])?;
        }
        Ok(old)
    }

    fn check_expected(&self, ctx: &WriteContext, sets: &[Arc<LeafSet>]) -> Result<()> {
        let Some(g) = ctx.expected_generation else {
            return Ok(());
        };
        if g >= self.generation {
            return Ok(());
        }
        if g < self.history.floor {
            return Err(Error::Conflict(
                "the view is too old to check for conflicts, please reload it".into(),
            ));
        }
        let n = self.dims.len();
        for op in self
            .history
            .ops
            .iter()
            .rev()
            .take_while(|op| op.generation > g)
        {
            for i in 0..op.len() {
                let coords = op.coords_of(i, n);
                if sets.iter().enumerate().all(|(d, s)| s.contains(coords[d])) {
                    return Err(Error::WriteConflict {
                        user: op.user.clone(),
                        cell: self.cell_name(coords),
                        value: self.store.get(coords),
                    });
                }
            }
        }
        Ok(())
    }

    fn check_permission(&self, ctx: &WriteContext, changes: &[Change]) -> Result<()> {
        let Some(allowed) = &ctx.can_write else {
            return Ok(());
        };
        match changes.iter().find(|c| !allowed(&c.coords)) {
            Some(c) => Err(Error::Denied(format!(
                "no write access to cell [{}]",
                self.cell_name(&c.coords)
            ))),
            None => Ok(()),
        }
    }

    fn equal_cells(
        &self,
        ctx: &WriteContext,
        sets: &[Arc<LeafSet>],
        target: f64,
    ) -> Result<Vec<(Vec<u32>, f64)>> {
        let n = area_len(sets);
        if n > ctx.max_equal_cells as u128 {
            return Err(Error::InvalidOperation(format!(
                "the area has {n} cells, the limit for an equal distribution is {}",
                ctx.max_equal_cells
            )));
        }
        let each = round_to(target / n as f64, ctx.decimals);
        Ok(combos(sets).into_iter().map(|c| (c, each)).collect())
    }

    /// Proportional to the existing values; equal if there are none or they sum to 0.
    fn proportional(
        &self,
        ctx: &WriteContext,
        sets: &[Arc<LeafSet>],
        existing: &[(Vec<u32>, f64)],
        target: f64,
    ) -> Result<Vec<(Vec<u32>, f64)>> {
        if target == 0.0 {
            return Ok(Vec::new());
        }
        let sum: f64 = existing.iter().map(|e| e.1).sum();
        if existing.is_empty() || sum.abs() < NEAR_ZERO {
            return self.equal_cells(ctx, sets, target);
        }
        let f = target / sum;
        Ok(existing
            .iter()
            .map(|(c, v)| (c.clone(), round_to(v * f, ctx.decimals)))
            .collect())
    }

    /// `dimension:member` or a plain member name that exists in exactly one dimension.
    fn resolve_reference(&self, name: &str) -> Result<(usize, MemberId)> {
        if let Some((dim, member)) = name.split_once(':')
            && let Some(d) = self
                .dims
                .iter()
                .position(|d| d.name().eq_ignore_ascii_case(dim.trim()))
        {
            return Ok((d, self.dims[d].member(member.trim())?));
        }
        let mut found = self
            .dims
            .iter()
            .enumerate()
            .filter_map(|(d, dim)| dim.resolve(name).map(|m| (d, m)));
        match (found.next(), found.next()) {
            (Some(hit), None) => Ok(hit),
            (None, _) => Err(Error::NotFound {
                kind: "member",
                name: name.to_string(),
            }),
            _ => Err(Error::Conflict(format!(
                "'{name}' exists in several dimensions, write 'dimension:{name}'"
            ))),
        }
    }

    /// Distribution proportional to another area. In every dimension named by the reference the
    /// base members of target and reference are matched by position (both must have equally many).
    fn reference_cells(
        &self,
        ctx: &WriteContext,
        sets: &[Arc<LeafSet>],
        names: &[String],
        target: f64,
    ) -> Result<Vec<(Vec<u32>, f64)>> {
        if target == 0.0 {
            return Ok(Vec::new());
        }
        let n = self.dims.len();
        let mut ref_sets: Vec<Arc<LeafSet>> = sets.to_vec();
        let mut maps: Vec<Option<HashMap<u32, u32>>> = vec![None; n];
        for name in names {
            let (d, m) = self.resolve_reference(name)?;
            let dim = &self.dims[d];
            if maps[d].is_some() {
                return Err(Error::InvalidOperation(format!(
                    "dimension '{}' is named twice in the reference",
                    dim.name()
                )));
            }
            let set = dim.leaf_set(m);
            let (from, to): (Vec<u32>, Vec<u32>) = (set.iter().collect(), sets[d].iter().collect());
            if set.has_weights() || from.len() != to.len() {
                return Err(Error::InvalidOperation(format!(
                    "reference '{name}' does not match the target in dimension '{}' ({} vs {} base members, weights must be 1)",
                    dim.name(),
                    from.len(),
                    to.len()
                )));
            }
            maps[d] = Some(from.into_iter().zip(to).collect());
            ref_sets[d] = set;
        }
        let mut weights: HashMap<Vec<u32>, f64> = HashMap::new();
        for row in self.rows_in(&ref_sets) {
            let row = row as usize;
            let coords: Vec<u32> = (0..n)
                .map(|d| {
                    let c = self.store.col(d)[row];
                    maps[d].as_ref().map_or(c, |m| m[&c])
                })
                .collect();
            *weights.entry(coords).or_insert(0.0) += self.store.value(row);
        }
        let total: f64 = weights.values().sum();
        if total.abs() < NEAR_ZERO {
            return Err(Error::InvalidOperation(
                "the reference area is empty or sums to zero".into(),
            ));
        }
        let f = target / total;
        Ok(weights
            .into_iter()
            .map(|(c, v)| (c, round_to(v * f, ctx.decimals)))
            .collect())
    }

    /// Writes `input` onto the cell at `addr` (base or aggregate). All changes of one call form one
    /// operation: it is applied completely or not at all.
    pub fn write(
        &mut self,
        ctx: &WriteContext,
        addr: &[MemberId],
        input: Input,
    ) -> Result<WriteResult> {
        let sets = self.area_sets(addr)?;
        if let Some(line) = self.rule_lock(addr, &sets) {
            return Err(Error::Denied(format!(
                "the area contains cells calculated by the rule in line {line}"
            )));
        }
        self.check_expected(ctx, &sets)?;
        let existing = self.existing_cells(&sets);
        let (news, target) = match input {
            Input::Clear => (Vec::new(), None),
            Input::Set(t) => (self.proportional(ctx, &sets, &existing, t)?, Some(t)),
            Input::Equal(t) if t == 0.0 => (Vec::new(), Some(t)),
            Input::Equal(t) => (self.equal_cells(ctx, &sets, t)?, Some(t)),
            Input::Delta(d) => {
                let t = existing.iter().map(|e| e.1).sum::<f64>() + d;
                (self.proportional(ctx, &sets, &existing, t)?, Some(t))
            }
            Input::Percent(p) => {
                let f = 1.0 + p / 100.0;
                (
                    existing
                        .iter()
                        .map(|(c, v)| (c.clone(), round_to(v * f, ctx.decimals)))
                        .collect(),
                    None,
                )
            }
            Input::Reference { members, target } => (
                self.reference_cells(ctx, &sets, &members, target)?,
                Some(target),
            ),
        };
        let changes = settle(ctx.decimals, target, news, existing);
        self.check_permission(ctx, &changes)?;
        self.commit(&ctx.user, "splash", true, changes)
    }

    /// Reverts the user's latest operation. Rejected if someone changed an affected cell since.
    pub fn undo(&mut self, ctx: &WriteContext) -> Result<WriteResult> {
        let n = self.dims.len();
        let id = self
            .history
            .undo
            .get(&ctx.user)
            .and_then(|s| s.last().copied())
            .ok_or_else(|| Error::InvalidOperation("nothing to undo".into()))?;
        let Ok(pos) = self.history.ops.binary_search_by_key(&id, |o| o.id) else {
            self.history.undo.remove(&ctx.user);
            return Err(Error::InvalidOperation(
                "the operation is no longer in the history".into(),
            ));
        };
        let op = &self.history.ops[pos];
        let mut changes = Vec::with_capacity(op.len());
        for i in 0..op.len() {
            let coords = op.coords_of(i, n);
            let current = self.store.get(coords);
            if current != dec(op.new[i]) {
                let user = self
                    .history
                    .last_toucher(pos, coords, n)
                    .unwrap_or("unknown")
                    .to_string();
                return Err(Error::WriteConflict {
                    user,
                    cell: self.cell_name(coords),
                    value: current,
                });
            }
            changes.push(Change {
                coords: coords.to_vec(),
                old: current,
                new: dec(op.old[i]),
            });
        }
        self.check_permission(ctx, &changes)?;
        let result = self.commit(&ctx.user, "undo", false, changes)?;
        if let Some(stack) = self.history.undo.get_mut(&ctx.user) {
            stack.pop();
        }
        Ok(result)
    }

    /// Applies the changes (all real changes) as one operation.
    fn commit(
        &mut self,
        user: &str,
        kind: &'static str,
        record_undo: bool,
        changes: Vec<Change>,
    ) -> Result<WriteResult> {
        if changes.is_empty() {
            return Ok(WriteResult {
                op_id: None,
                generation: self.generation,
                cells_changed: 0,
            });
        }
        let n = self.dims.len();
        let id = self.history.next_id;
        let generation = self.generation + 1;
        if self.sink.is_some() {
            let record = OpRecord {
                id,
                generation,
                user: user.to_string(),
                kind,
                changes: changes
                    .iter()
                    .map(|c| CellChange {
                        addr: self.coords_to_members(&c.coords),
                        old: c.old,
                        new: c.new,
                    })
                    .collect(),
            };
            let name = self.name.clone();
            if let Some(sink) = self.sink.as_mut() {
                sink.record(&name, &record)?;
            }
        }
        self.generation = generation;
        self.cache.lock().clear();
        self.history.next_id += 1;
        let mut op = Op {
            id,
            generation: self.generation,
            user: user.to_string(),
            coords: Vec::with_capacity(changes.len() * n),
            old: Vec::with_capacity(changes.len()),
            new: Vec::with_capacity(changes.len()),
        };
        for c in changes {
            match c.new {
                Some(v) => {
                    self.store.set(&c.coords, v);
                }
                None => {
                    self.store.remove(&c.coords);
                }
            }
            op.coords.extend_from_slice(&c.coords);
            op.old.push(enc(c.old));
            op.new.push(enc(c.new));
        }
        let cells_changed = op.len();
        self.history.cells += cells_changed;
        self.history.ops.push_back(op);
        while self.history.cells > MAX_LOG_CELLS && self.history.ops.len() > 1 {
            if let Some(dropped) = self.history.ops.pop_front() {
                self.history.cells -= dropped.len();
                self.history.floor = dropped.generation;
            }
        }
        if record_undo {
            let stack = self.history.undo.entry(user.to_string()).or_default();
            stack.push(id);
            if stack.len() > UNDO_DEPTH {
                stack.remove(0);
            }
        }
        Ok(WriteResult {
            op_id: Some(id),
            generation: self.generation,
            cells_changed,
        })
    }
}
