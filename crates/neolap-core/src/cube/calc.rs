//! Cell evaluation with rules.
//!
//! `value()` aggregates stored base cells only. `cell()` additionally applies the rules of the
//! cube: the first matching rule calculates a cell; aggregates of `N:` rules sum the rule results
//! of their base cells (candidates come from the feeders); errors become cell values.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rayon::prelude::*;

use super::{Cube, SCAN_CHUNK};
use crate::dimension::{AttrValue, LeafSet};
use crate::rules::{BinOp, CellRef, Expr, Rule, RuleSet, RuleStatus, Scope, UnOp};
use crate::{MemberId, Result};

/// Recursion depth after which a cell evaluates to `#CYCLE`.
const MAX_DEPTH: usize = 64;

/// Error values of cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellError {
    Div0,
    Ref,
    Err,
    Cycle,
}

impl std::fmt::Display for CellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CellError::Div0 => "#DIV0",
            CellError::Ref => "#REF",
            CellError::Err => "#ERR",
            CellError::Cycle => "#CYCLE",
        })
    }
}

/// Result of evaluating a cell.
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    Empty,
    Num(f64),
    Text(String),
    Error(CellError),
}

impl CellValue {
    pub fn number(&self) -> Option<f64> {
        match self {
            CellValue::Num(v) => Some(*v),
            _ => None,
        }
    }
}

/// Gives rules access to other cubes (`DB(...)`).
pub trait CubeProvider {
    fn cube(&self, name: &str) -> Option<&Cube>;
}

impl CubeProvider for HashMap<String, Cube> {
    fn cube(&self, name: &str) -> Option<&Cube> {
        self.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, c)| c)
    }
}

impl CubeProvider for [Cube] {
    fn cube(&self, name: &str) -> Option<&Cube> {
        self.iter().find(|c| c.name().eq_ignore_ascii_case(name))
    }
}

struct Ctx<'a> {
    depth: usize,
    db: Option<&'a dyn CubeProvider>,
}

enum Val {
    Num(f64),
    Str(String),
    Null,
    Err(CellError),
    Continue,
}

impl Val {
    fn into_cell(self) -> CellValue {
        match self {
            Val::Num(v) if v.is_finite() => CellValue::Num(v),
            Val::Num(_) | Val::Continue => CellValue::Error(CellError::Err),
            Val::Str(s) => CellValue::Text(s),
            Val::Null => CellValue::Empty,
            Val::Err(e) => CellValue::Error(e),
        }
    }

    fn from_cell(c: CellValue) -> Val {
        match c {
            CellValue::Empty => Val::Null,
            CellValue::Num(v) => Val::Num(v),
            CellValue::Text(s) => Val::Str(s),
            CellValue::Error(e) => Val::Err(e),
        }
    }

    /// Number for arithmetic: empty counts as 0.
    fn num(self) -> std::result::Result<f64, Val> {
        match self {
            Val::Num(v) => Ok(v),
            Val::Null => Ok(0.0),
            Val::Err(e) => Err(Val::Err(e)),
            Val::Str(_) | Val::Continue => Err(Val::Err(CellError::Err)),
        }
    }
}

fn member_text(v: Val) -> std::result::Result<String, Val> {
    match v {
        Val::Str(s) => Ok(s),
        Val::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => Ok(format!("{}", n as i64)),
        Val::Num(n) => Ok(n.to_string()),
        Val::Err(e) => Err(Val::Err(e)),
        Val::Null | Val::Continue => Err(Val::Err(CellError::Ref)),
    }
}

macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(v) => return v,
        }
    };
}

impl Cube {
    /// Replaces the rule text. Syntax errors reject the text (with position); rules that do not
    /// resolve against the model are marked invalid and skipped.
    pub fn set_rules(&mut self, text: &str) -> Result<Vec<RuleStatus>> {
        let set = RuleSet::build(text, &self.dims)?;
        let report = set.report();
        self.rules = set;
        self.generation += 1;
        self.cache.lock().clear();
        Ok(report)
    }

    pub fn rules_text(&self) -> &str {
        &self.rules.text
    }

    pub fn rule_status(&self) -> Vec<RuleStatus> {
        self.rules.report()
    }

    pub(crate) fn valid_rules(&self) -> impl Iterator<Item = &Rule> {
        self.rules
            .entries
            .iter()
            .filter_map(|e| e.rule.as_ref().ok())
    }

    pub(super) fn is_base_addr(&self, addr: &[MemberId]) -> bool {
        self.dims.iter().zip(addr).all(|(d, &m)| d.is_leaf(m))
    }

    fn applies(rule: &Rule, base: bool, addr: &[MemberId]) -> bool {
        match rule.scope {
            Scope::All => rule.matches(addr),
            Scope::Base => base && rule.matches(addr),
            Scope::Aggregate => !base && rule.matches(addr),
        }
    }

    /// Line of a rule that calculates this cell or any base cell of the area (writes are locked).
    pub(super) fn rule_lock(&self, addr: &[MemberId], sets: &[Arc<LeafSet>]) -> Option<usize> {
        let base = self.is_base_addr(addr);
        self.valid_rules()
            .find(|r| {
                Self::applies(r, base, addr) || (r.scope != Scope::Aggregate && r.intersects(sets))
            })
            .map(|r| r.line)
    }

    /// Line of the rule that calculates this base cell (ordinal per dimension), if any.
    pub(crate) fn rule_lock_base(&self, coords: &[u32]) -> Option<usize> {
        self.valid_rules()
            .find(|r| r.scope != Scope::Aggregate && r.base_match(|d| coords[d]))
            .map(|r| r.line)
    }

    /// Whether a rule influences this cell (directly or through base cells below it).
    pub(crate) fn rule_touches(&self, addr: &[MemberId]) -> bool {
        if self.rules.is_empty() {
            return false;
        }
        let sets: Vec<Arc<LeafSet>> = self
            .dims
            .iter()
            .zip(addr)
            .map(|(d, &m)| d.leaf_set(m))
            .collect();
        self.rule_lock(addr, &sets).is_some()
    }

    /// Evaluates a cell (base or aggregate) with rules. `db` gives access to other cubes.
    pub fn cell(&self, addr: &[MemberId], db: Option<&dyn CubeProvider>) -> Result<CellValue> {
        self.check_addr(addr)?;
        Ok(self.eval_cell(addr, &mut Ctx { depth: 0, db }))
    }

    fn eval_cell(&self, addr: &[MemberId], ctx: &mut Ctx) -> CellValue {
        if ctx.depth > MAX_DEPTH {
            return CellValue::Error(CellError::Cycle);
        }
        let base = self.is_base_addr(addr);
        for rule in self.valid_rules() {
            if !Self::applies(rule, base, addr) {
                continue;
            }
            ctx.depth += 1;
            let v = self.eval(&rule.expr, addr, ctx);
            ctx.depth -= 1;
            if !matches!(v, Val::Continue) {
                return v.into_cell();
            }
        }
        if base {
            let coords: Vec<u32> = self
                .dims
                .iter()
                .zip(addr)
                .filter_map(|(d, &m)| d.leaf_ordinal(m))
                .collect();
            return self
                .store
                .get(&coords)
                .map_or(CellValue::Empty, CellValue::Num);
        }
        let sets: Vec<Arc<LeafSet>> = self
            .dims
            .iter()
            .zip(addr)
            .map(|(d, &m)| d.leaf_set(m))
            .collect();
        let rules: Vec<&Rule> = self
            .valid_rules()
            .filter(|r| r.scope != Scope::Aggregate && r.intersects(&sets))
            .collect();
        if rules.is_empty() {
            return self
                .value(addr)
                .ok()
                .flatten()
                .map_or(CellValue::Empty, CellValue::Num);
        }
        self.rule_aggregate(&sets, &rules, ctx)
    }

    /// Aggregate over an area that contains rule cells: stored cells that no rule covers are summed
    /// directly, the others (and the cells fed by the feeders) are evaluated one by one.
    fn rule_aggregate(&self, sets: &[Arc<LeafSet>], rules: &[&Rule], ctx: &mut Ctx) -> CellValue {
        let n = self.dims.len();
        let store = &self.store;
        let in_area = |row: usize| (0..n).all(|d| sets[d].contains(store.col(d)[row]));
        let weight = |coords: &dyn Fn(usize) -> u32| -> f64 {
            (0..n)
                .map(|d| f64::from(sets[d].weight(coords(d))))
                .product()
        };

        let (mut sum, mut hit, covered): (f64, bool, Vec<Vec<u32>>) = (0..store.n_rows())
            .step_by(SCAN_CHUNK)
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|start| {
                let (mut sum, mut hit, mut covered) = (0.0, false, Vec::new());
                for row in start..(start + SCAN_CHUNK).min(store.n_rows()) {
                    if !store.is_live(row) || !in_area(row) {
                        continue;
                    }
                    if rules.iter().any(|r| r.base_match(|d| store.col(d)[row])) {
                        covered.push((0..n).map(|d| store.col(d)[row]).collect());
                    } else {
                        sum += store.value(row) * weight(&|d| store.col(d)[row]);
                        hit = true;
                    }
                }
                (sum, hit, covered)
            })
            .reduce(
                || (0.0, false, Vec::new()),
                |mut a, mut b| {
                    a.2.append(&mut b.2);
                    (a.0 + b.0, a.1 | b.1, a.2)
                },
            );

        let mut cand: HashSet<Vec<u32>> = covered.into_iter().collect();
        for rule in rules {
            for feeder in &rule.feeders {
                cand.extend(self.fed_cells(rule, feeder, sets));
            }
        }
        let mut cand: Vec<Vec<u32>> = cand.into_iter().collect();
        cand.sort();
        for coords in cand {
            let addr: Vec<MemberId> = self
                .dims
                .iter()
                .zip(&coords)
                .filter_map(|(d, &o)| d.leaf_member(o))
                .collect();
            ctx.depth += 1;
            let v = self.eval_cell(&addr, ctx);
            ctx.depth -= 1;
            match v {
                CellValue::Num(v) => {
                    sum += v * weight(&|d| coords[d]);
                    hit = true;
                }
                CellValue::Empty => {}
                CellValue::Text(_) => return CellValue::Error(CellError::Err),
                e @ CellValue::Error(_) => return e,
            }
        }
        if hit {
            CellValue::Num(sum)
        } else {
            CellValue::Empty
        }
    }

    /// Target cells of `rule` (base cells in the area) whose feeder cell holds a stored value.
    pub(crate) fn fed_cells(
        &self,
        rule: &Rule,
        feeder: &CellRef,
        sets: &[Arc<LeafSet>],
    ) -> Vec<Vec<u32>> {
        let n = self.dims.len();
        let mut fed_dims: Vec<(usize, Arc<LeafSet>)> = Vec::new();
        for it in &feeder.items {
            match it.fixed {
                Some(m) => fed_dims.push((it.dim, self.dims[it.dim].leaf_set(m))),
                None => return Vec::new(), // computed members cannot be inverted
            }
        }
        // Dimension d of the feeder cell replaces the target coordinate if the rule fixes it.
        let replace: Vec<Option<u32>> = (0..n)
            .map(
                |d| match (&rule.base_ords[d], fed_dims.iter().any(|f| f.0 == d)) {
                    (Some(s), true) if s.len() == 1 => s.iter().next().copied(),
                    _ => None,
                },
            )
            .collect();
        let store = &self.store;
        (0..store.n_rows())
            .step_by(SCAN_CHUNK)
            .collect::<Vec<_>>()
            .into_par_iter()
            .flat_map_iter(|start| {
                let mut out = Vec::new();
                for row in start..(start + SCAN_CHUNK).min(store.n_rows()) {
                    if !store.is_live(row)
                        || !fed_dims.iter().all(|(d, s)| s.contains(store.col(*d)[row]))
                    {
                        continue;
                    }
                    let at = |d: usize| replace[d].unwrap_or(store.col(d)[row]);
                    if (0..n).all(|d| sets[d].contains(at(d))) && rule.base_match(at) {
                        out.push((0..n).map(at).collect::<Vec<u32>>());
                    }
                }
                out
            })
            .collect()
    }

    fn eval_num(
        &self,
        e: &Expr,
        addr: &[MemberId],
        ctx: &mut Ctx,
    ) -> std::result::Result<f64, Val> {
        self.eval(e, addr, ctx).num()
    }

    fn eval_name(
        &self,
        e: &Expr,
        addr: &[MemberId],
        ctx: &mut Ctx,
    ) -> std::result::Result<String, Val> {
        member_text(self.eval(e, addr, ctx))
    }

    fn eval(&self, e: &Expr, addr: &[MemberId], ctx: &mut Ctx) -> Val {
        match e {
            Expr::Num(v) => Val::Num(*v),
            Expr::Str(s) => Val::Str(s.clone()),
            Expr::Null => Val::Null,
            Expr::Continue => Val::Continue,
            Expr::Dim(_, idx) => match self.dims[*idx].member_name(addr[*idx]) {
                Some(n) => Val::Str(n.to_string()),
                None => Val::Err(CellError::Ref),
            },
            Expr::Cell(r) => self.eval_ref(r, addr, ctx),
            Expr::Unary(UnOp::Neg, x) => Val::Num(-tri!(self.eval_num(x, addr, ctx))),
            Expr::Unary(UnOp::Not, x) => {
                Val::Num(f64::from(tri!(self.eval_num(x, addr, ctx)) == 0.0))
            }
            Expr::Bin(op, a, b) => self.eval_bin(*op, a, b, addr, ctx),
            Expr::Call { name, args, .. } => self.eval_call(name, args, addr, ctx),
        }
    }

    fn eval_bin(&self, op: BinOp, a: &Expr, b: &Expr, addr: &[MemberId], ctx: &mut Ctx) -> Val {
        match op {
            BinOp::And => {
                let l = tri!(self.eval_num(a, addr, ctx));
                if l == 0.0 {
                    return Val::Num(0.0);
                }
                Val::Num(f64::from(tri!(self.eval_num(b, addr, ctx)) != 0.0))
            }
            BinOp::Or => {
                let l = tri!(self.eval_num(a, addr, ctx));
                if l != 0.0 {
                    return Val::Num(1.0);
                }
                Val::Num(f64::from(tri!(self.eval_num(b, addr, ctx)) != 0.0))
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let (l, r) = (self.eval(a, addr, ctx), self.eval(b, addr, ctx));
                let ord = match (l, r) {
                    (Val::Err(e), _) | (_, Val::Err(e)) => return Val::Err(e),
                    (Val::Str(x), Val::Str(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
                    (Val::Str(x), Val::Null) => x.cmp(&String::new()),
                    (Val::Null, Val::Str(y)) => String::new().cmp(&y),
                    (Val::Str(_), _) | (_, Val::Str(_)) => {
                        return match op {
                            BinOp::Eq => Val::Num(0.0),
                            BinOp::Ne => Val::Num(1.0),
                            _ => Val::Err(CellError::Err),
                        };
                    }
                    (l, r) => tri!(l.num())
                        .partial_cmp(&tri!(r.num()))
                        .unwrap_or(std::cmp::Ordering::Equal),
                };
                use std::cmp::Ordering::*;
                Val::Num(f64::from(match op {
                    BinOp::Eq => ord == Equal,
                    BinOp::Ne => ord != Equal,
                    BinOp::Lt => ord == Less,
                    BinOp::Le => ord != Greater,
                    BinOp::Gt => ord == Greater,
                    _ => ord != Less,
                }))
            }
            _ => {
                let l = tri!(self.eval_num(a, addr, ctx));
                let r = tri!(self.eval_num(b, addr, ctx));
                match op {
                    BinOp::Add => Val::Num(l + r),
                    BinOp::Sub => Val::Num(l - r),
                    BinOp::Mul => Val::Num(l * r),
                    BinOp::Div if r == 0.0 => Val::Err(CellError::Div0),
                    BinOp::Div => Val::Num(l / r),
                    _ => {
                        let v = l.powf(r);
                        if v.is_finite() {
                            Val::Num(v)
                        } else {
                            Val::Err(CellError::Err)
                        }
                    }
                }
            }
        }
    }

    fn eval_ref(&self, r: &CellRef, addr: &[MemberId], ctx: &mut Ctx) -> Val {
        let mut target = addr.to_vec();
        for it in &r.items {
            target[it.dim] = match it.fixed {
                Some(m) => m,
                None => {
                    let name = tri!(self.eval_name(&it.member, addr, ctx));
                    match self.dims[it.dim].resolve(&name) {
                        Some(m) => m,
                        None => return Val::Err(CellError::Ref),
                    }
                }
            };
        }
        ctx.depth += 1;
        let v = self.eval_cell(&target, ctx);
        ctx.depth -= 1;
        Val::from_cell(v)
    }

    fn dim_by_name(&self, name: &str) -> Option<usize> {
        self.dims
            .iter()
            .position(|d| d.name().eq_ignore_ascii_case(name))
    }

    fn eval_call(&self, name: &str, args: &[Expr], addr: &[MemberId], ctx: &mut Ctx) -> Val {
        let nums = |this: &Self, ctx: &mut Ctx| -> std::result::Result<Vec<f64>, Val> {
            let mut out = Vec::new();
            for a in args {
                match this.eval(a, addr, ctx) {
                    Val::Null => {}
                    v => out.push(v.num()?),
                }
            }
            Ok(out)
        };
        match name {
            "IF" => {
                let cond = match self.eval(&args[0], addr, ctx) {
                    Val::Null => false,
                    v => tri!(v.num()) != 0.0,
                };
                match (cond, args.get(2)) {
                    (true, _) => self.eval(&args[1], addr, ctx),
                    (false, Some(e)) => self.eval(e, addr, ctx),
                    (false, None) => Val::Null,
                }
            }
            "ISEMPTY" => match self.eval(&args[0], addr, ctx) {
                Val::Null => Val::Num(1.0),
                Val::Str(s) => Val::Num(f64::from(s.is_empty())),
                Val::Err(e) => Val::Err(e),
                _ => Val::Num(0.0),
            },
            "MIN" | "MAX" => {
                let v = tri!(nums(self, ctx));
                let pick = if name == "MIN" { f64::min } else { f64::max };
                v.into_iter().reduce(pick).map_or(Val::Null, Val::Num)
            }
            "ABS" => Val::Num(tri!(self.eval_num(&args[0], addr, ctx)).abs()),
            "SQRT" => {
                let x = tri!(self.eval_num(&args[0], addr, ctx));
                if x < 0.0 {
                    Val::Err(CellError::Err)
                } else {
                    Val::Num(x.sqrt())
                }
            }
            "LN" => {
                let x = tri!(self.eval_num(&args[0], addr, ctx));
                if x <= 0.0 {
                    Val::Err(CellError::Err)
                } else {
                    Val::Num(x.ln())
                }
            }
            "EXP" => {
                let v = tri!(self.eval_num(&args[0], addr, ctx)).exp();
                if v.is_finite() {
                    Val::Num(v)
                } else {
                    Val::Err(CellError::Err)
                }
            }
            "ROUND" => {
                let x = tri!(self.eval_num(&args[0], addr, ctx));
                let d = match args.get(1) {
                    Some(e) => tri!(self.eval_num(e, addr, ctx)),
                    None => 0.0,
                };
                let f = 10f64.powi(d as i32);
                Val::Num((x * f).round() / f)
            }
            "ATTR" => {
                let dim = tri!(self.eval_name(&args[0], addr, ctx));
                let member = tri!(self.eval_name(&args[1], addr, ctx));
                let attr = tri!(self.eval_name(&args[2], addr, ctx));
                let Some(d) = self.dim_by_name(&dim) else {
                    return Val::Err(CellError::Ref);
                };
                let Some(m) = self.dims[d].resolve(&member) else {
                    return Val::Err(CellError::Ref);
                };
                match self.dims[d].attribute(&attr, m) {
                    Some(AttrValue::Number(v)) => Val::Num(*v),
                    Some(AttrValue::Text(s)) => Val::Str(s.clone()),
                    None => Val::Null,
                }
            }
            "DB" => {
                let cube_name = tri!(self.eval_name(&args[0], addr, ctx));
                let other: &Cube = if cube_name.eq_ignore_ascii_case(&self.name) {
                    self
                } else {
                    match ctx.db.and_then(|p| p.cube(&cube_name)) {
                        Some(c) => c,
                        None => return Val::Err(CellError::Ref),
                    }
                };
                if args.len() - 1 != other.dims.len() {
                    return Val::Err(CellError::Ref);
                }
                let mut target = Vec::with_capacity(other.dims.len());
                for (d, a) in other.dims.iter().zip(&args[1..]) {
                    let member = tri!(self.eval_name(a, addr, ctx));
                    match d.resolve(&member) {
                        Some(m) => target.push(m),
                        None => return Val::Err(CellError::Ref),
                    }
                }
                ctx.depth += 1;
                let v = other.eval_cell(&target, ctx);
                ctx.depth -= 1;
                Val::from_cell(v)
            }
            "ELEMLEVEL" | "ISLEAF" | "PARENT" => {
                let dim = tri!(self.eval_name(&args[0], addr, ctx));
                let Some(d) = self.dim_by_name(&dim) else {
                    return Val::Err(CellError::Ref);
                };
                let m = addr[d];
                match name {
                    "ELEMLEVEL" => Val::Num(f64::from(self.dims[d].level(m))),
                    "ISLEAF" => Val::Num(f64::from(self.dims[d].is_leaf(m))),
                    _ => match self.dims[d].parents(m).first() {
                        Some(&(p, _)) => {
                            Val::Str(self.dims[d].member_name(p).unwrap_or_default().to_string())
                        }
                        None => Val::Null,
                    },
                }
            }
            _ => Val::Err(CellError::Err),
        }
    }
}
