//! Rule language: parsing, name resolution, compiled rules.
//!
//! Syntax errors reject the whole text with a position; rules that cannot be resolved against the
//! model (unknown member, …) are marked invalid and skipped, so they never block the model.
//! See `docs/Implementierungsplan.md`, chapter 4.4.

mod lexer;
mod parser;

use std::collections::HashSet;
use std::sync::Arc;

use crate::dimension::Dimension;
use crate::{MemberId, Result};

pub use lexer::Pos;
pub use parser::{BinOp, CellRef, Expr, RefItem, RuleAst, Scope, UnOp};

/// Selection of members in a rule target.
pub enum TargetSel {
    /// The member itself (an aggregate means the aggregate cell, not its children).
    Exact(MemberId),
    /// `DESC('x')`: all descendants of x.
    Desc(HashSet<MemberId>),
}

/// A rule resolved against the dimensions of a cube.
pub struct Rule {
    pub line: usize,
    pub scope: Scope,
    /// Per dimension the target selection; `None` = all members.
    pub target: Vec<Option<TargetSel>>,
    /// Per dimension the base-member ordinals the target selects; `None` = all.
    pub base_ords: Vec<Option<HashSet<u32>>>,
    pub expr: Expr,
    /// Cells whose non-emptiness makes the target non-empty (explicit `FEED` or derived).
    pub feeders: Vec<CellRef>,
}

impl Rule {
    /// Whether the target pattern selects exactly this cell.
    pub fn matches(&self, addr: &[MemberId]) -> bool {
        self.target.iter().enumerate().all(|(d, t)| match t {
            None => true,
            Some(TargetSel::Exact(m)) => addr[d] == *m,
            Some(TargetSel::Desc(set)) => set.contains(&addr[d]),
        })
    }

    /// Whether the pattern selects this base cell (leaf ordinal per dimension).
    pub fn base_match(&self, ord_of: impl Fn(usize) -> u32) -> bool {
        self.base_ords
            .iter()
            .enumerate()
            .all(|(d, o)| o.as_ref().is_none_or(|s| s.contains(&ord_of(d))))
    }

    /// Whether any base cell of the area (leaf sets per dimension) can be selected by the pattern.
    pub fn intersects(&self, sets: &[Arc<crate::dimension::LeafSet>]) -> bool {
        self.base_ords.iter().enumerate().all(|(d, o)| match o {
            None => !sets[d].is_empty(),
            Some(s) => s.iter().any(|&x| sets[d].contains(x)),
        })
    }
}

pub struct RuleEntry {
    pub line: usize,
    pub rule: std::result::Result<Rule, String>,
}

/// Status of one rule after saving, for the editor.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleStatus {
    pub line: usize,
    pub valid: bool,
    pub message: Option<String>,
}

#[derive(Default)]
pub struct RuleSet {
    pub(crate) text: String,
    pub(crate) entries: Vec<RuleEntry>,
}

/// Function name → (min, max) argument count.
const FUNCTIONS: &[(&str, usize, usize)] = &[
    ("IF", 2, 3),
    ("ISEMPTY", 1, 1),
    ("MIN", 1, usize::MAX),
    ("MAX", 1, usize::MAX),
    ("ABS", 1, 1),
    ("ROUND", 1, 2),
    ("SQRT", 1, 1),
    ("LN", 1, 1),
    ("EXP", 1, 1),
    ("ATTR", 3, 3),
    ("DB", 2, usize::MAX),
    ("ELEMLEVEL", 1, 1),
    ("ISLEAF", 1, 1),
    ("PARENT", 1, 1),
    ("DESC", 1, 1),
];

fn walk<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    f(e);
    match e {
        Expr::Unary(_, x) => walk(x, f),
        Expr::Bin(_, a, b) => {
            walk(a, f);
            walk(b, f);
        }
        Expr::Call { args, .. } => args.iter().for_each(|a| walk(a, f)),
        Expr::Cell(r) => r.items.iter().for_each(|i| walk(&i.member, f)),
        _ => {}
    }
}

fn dim_index(dims: &[Arc<Dimension>], name: &str) -> Option<usize> {
    let n = name.to_lowercase();
    dims.iter().position(|d| d.name().to_lowercase() == n)
}

fn descendants(dim: &Dimension, m: MemberId) -> HashSet<MemberId> {
    let mut out = HashSet::new();
    let mut stack: Vec<MemberId> = dim.children(m).iter().map(|c| c.0).collect();
    while let Some(v) = stack.pop() {
        if out.insert(v) {
            stack.extend(dim.children(v).iter().map(|c| c.0));
        }
    }
    out
}

fn resolve_expr(e: &mut Expr, dims: &[Arc<Dimension>]) -> std::result::Result<(), String> {
    match e {
        Expr::Dim(name, idx) => {
            *idx = dim_index(dims, name).ok_or_else(|| format!("dimension '{name}' not found"))?;
        }
        Expr::Cell(r) => resolve_ref(r, dims, false)?,
        Expr::Unary(_, x) => resolve_expr(x, dims)?,
        Expr::Bin(_, a, b) => {
            resolve_expr(a, dims)?;
            resolve_expr(b, dims)?;
        }
        Expr::Call { name, args, pos } => {
            if name == "DESC" {
                return Err(format!("{pos}: DESC is only valid in rule targets"));
            }
            let Some(&(_, min, max)) = FUNCTIONS.iter().find(|f| f.0 == name) else {
                return Err(format!("{pos}: unknown function {name}"));
            };
            if args.len() < min || args.len() > max {
                return Err(format!(
                    "{pos}: {name} takes {min}{} argument(s), got {}",
                    if max == min {
                        String::new()
                    } else if max == usize::MAX {
                        " or more".into()
                    } else {
                        format!(" to {max}")
                    },
                    args.len()
                ));
            }
            for a in args {
                resolve_expr(a, dims)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn resolve_ref(
    r: &mut CellRef,
    dims: &[Arc<Dimension>],
    target: bool,
) -> std::result::Result<(), String> {
    let mut seen = HashSet::new();
    for it in &mut r.items {
        // literal member name (or the argument of DESC in a target)
        let literal = match &it.member {
            Expr::Str(s) => Some((s.clone(), false)),
            Expr::Call { name, args, .. } if target && name == "DESC" => match args.as_slice() {
                [Expr::Str(s)] => Some((s.clone(), true)),
                _ => return Err(format!("{}: DESC needs one member name", r.pos)),
            },
            _ => None,
        };
        it.dim = match (&it.dim_name, &literal) {
            (Some(n), _) => {
                dim_index(dims, n).ok_or_else(|| format!("{}: dimension '{n}' not found", r.pos))?
            }
            (None, Some((m, _))) => {
                let hits: Vec<usize> = (0..dims.len())
                    .filter(|&d| dims[d].resolve(m).is_some())
                    .collect();
                match hits.as_slice() {
                    [d] => *d,
                    [] => {
                        return Err(format!(
                            "{}: member '{m}' not found in any dimension",
                            r.pos
                        ));
                    }
                    _ => {
                        return Err(format!(
                            "{}: member '{m}' exists in several dimensions, write dimension:'{m}'",
                            r.pos
                        ));
                    }
                }
            }
            (None, None) => {
                return Err(format!(
                    "{}: a computed member needs a dimension (dimension:expression)",
                    r.pos
                ));
            }
        };
        if !seen.insert(it.dim) {
            return Err(format!(
                "{}: dimension '{}' is named twice",
                r.pos,
                dims[it.dim].name()
            ));
        }
        match literal {
            Some((m, desc)) => {
                it.fixed = Some(dims[it.dim].resolve(&m).ok_or_else(|| {
                    format!(
                        "{}: member '{m}' not found in dimension '{}'",
                        r.pos,
                        dims[it.dim].name()
                    )
                })?);
                it.desc = desc;
            }
            None if target => {
                return Err(format!(
                    "{}: the target of a rule must name its members literally",
                    r.pos
                ));
            }
            None => resolve_expr(&mut it.member, dims)?,
        }
    }
    Ok(())
}

fn compile(mut ast: RuleAst, dims: &[Arc<Dimension>]) -> std::result::Result<Rule, String> {
    resolve_ref(&mut ast.target, dims, true)?;
    resolve_expr(&mut ast.expr, dims)?;
    for f in &mut ast.feed {
        resolve_ref(f, dims, false)?;
    }
    let n = dims.len();
    let mut target: Vec<Option<TargetSel>> = (0..n).map(|_| None).collect();
    let mut base_ords: Vec<Option<HashSet<u32>>> = vec![None; n];
    for it in &ast.target.items {
        let (d, m) = (it.dim, it.fixed.expect("resolved"));
        let dim = &dims[d];
        if it.desc {
            let set = descendants(dim, m);
            base_ords[d] = Some(set.iter().filter_map(|&x| dim.leaf_ordinal(x)).collect());
            target[d] = Some(TargetSel::Desc(set));
        } else {
            base_ords[d] = Some(dim.leaf_ordinal(m).into_iter().collect());
            target[d] = Some(TargetSel::Exact(m));
        }
    }
    let feeders = if ast.feed.is_empty() {
        // derived feeder: every cell reference of the formula
        let mut refs = Vec::new();
        walk(&ast.expr, &mut |e| {
            if let Expr::Cell(r) = e {
                refs.push(r.clone());
            }
        });
        refs
    } else {
        ast.feed
    };
    Ok(Rule {
        line: ast.line,
        scope: ast.scope,
        target,
        base_ords,
        expr: ast.expr,
        feeders,
    })
}

impl RuleSet {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Parses and resolves the text. Only syntax errors are returned as `Err`.
    pub fn build(text: &str, dims: &[Arc<Dimension>]) -> Result<RuleSet> {
        let asts = parser::parse(text)?;
        let mut entries = Vec::with_capacity(asts.len());
        for ast in asts {
            let line = ast.line;
            entries.push(RuleEntry {
                line,
                rule: compile(ast, dims),
            });
        }
        Ok(RuleSet {
            text: text.to_string(),
            entries,
        })
    }

    pub fn report(&self) -> Vec<RuleStatus> {
        self.entries
            .iter()
            .map(|e| match &e.rule {
                Ok(_) => RuleStatus {
                    line: e.line,
                    valid: true,
                    message: None,
                },
                Err(m) => RuleStatus {
                    line: e.line,
                    valid: false,
                    message: Some(m.clone()),
                },
            })
            .collect()
    }
}
