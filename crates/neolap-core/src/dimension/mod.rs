//! Dimensions: members, hierarchies with weights, aliases, attributes, subsets.
//!
//! A [`Dimension`] is an immutable, query-optimised snapshot. Changes go through a
//! [`DimensionEditor`] whose `commit()` validates the result (cycles, name conflicts,
//! contradicting path weights) and recomputes `level`, `depth` and the DFS numbering of the
//! base members. See `docs/Implementierungsplan.md`, chapter 4.1.

mod editor;
mod import;
mod leafset;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::MemberId;

pub use editor::DimensionEditor;
pub use import::{
    ImportMode, ImportReport, import_levels, import_parent_child, import_parent_child_with,
};
pub use leafset::{LeafSet, LeafShape};

/// Number of leaf sets kept in the cache of one dimension.
const LEAF_CACHE_CAPACITY: usize = 4096;

/// Marker for "this member has no leaf ordinal" (aggregates, deleted slots).
const NO_ORD: u32 = u32::MAX;

/// Purpose of a dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DimensionKind {
    #[default]
    Generic,
    Time,
    Version,
    Measures,
}

/// Value of a member attribute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AttrValue {
    Number(f64),
    Text(String),
}

/// A named list of members (selection for views).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subset {
    pub name: String,
    pub members: Vec<MemberId>,
}

/// Editable raw state; everything else in [`Dimension`] is derived from it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Raw {
    /// Index = `MemberId`; `None` marks a deleted slot.
    pub names: Vec<Option<String>>,
    pub aliases: Vec<Vec<String>>,
    /// Ordered children with edge weight.
    pub children: Vec<Vec<(MemberId, f32)>>,
    /// Lower-case names and aliases → member.
    pub lookup: HashMap<String, MemberId>,
    pub attributes: BTreeMap<String, HashMap<MemberId, AttrValue>>,
    pub subsets: Vec<Subset>,
    pub free: Vec<MemberId>,
}

/// Serializable form of a dimension (persistence).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionData {
    pub name: String,
    pub kind: DimensionKind,
    /// Index = `MemberId`; `null` marks a free slot.
    pub names: Vec<Option<String>>,
    pub aliases: Vec<Vec<String>>,
    pub children: Vec<Vec<(MemberId, f32)>>,
    pub attributes: BTreeMap<String, Vec<(MemberId, AttrValue)>>,
    pub subsets: Vec<Subset>,
    pub free: Vec<MemberId>,
}

/// An immutable dimension snapshot.
pub struct Dimension {
    name: String,
    kind: DimensionKind,
    raw: Raw,
    parents: Vec<Vec<(MemberId, f32)>>,
    roots: Vec<MemberId>,
    /// 0 = base member, otherwise height above the base.
    level: Vec<u8>,
    /// Shortest distance from a root.
    depth: Vec<u8>,
    /// DFS ordinal of base members, `NO_ORD` otherwise.
    leaf_ord: Vec<u32>,
    /// Ordinal → member.
    leaf_members: Vec<MemberId>,
    /// Sorted (lower-case text, member) over names and aliases.
    search_index: Vec<(String, MemberId)>,
    leaf_cache: Mutex<HashMap<MemberId, Arc<LeafSet>>>,
    live: usize,
}

impl Dimension {
    /// An empty dimension.
    pub fn new(name: impl Into<String>, kind: DimensionKind) -> Dimension {
        DimensionEditor::new(name.into(), kind, Raw::default())
            .commit()
            .expect("an empty dimension is always valid")
    }

    /// The persistent form of this dimension.
    pub fn to_data(&self) -> DimensionData {
        let mut attributes = BTreeMap::new();
        for (name, values) in &self.raw.attributes {
            let mut v: Vec<(MemberId, AttrValue)> =
                values.iter().map(|(k, a)| (*k, a.clone())).collect();
            v.sort_by_key(|e| e.0);
            attributes.insert(name.clone(), v);
        }
        DimensionData {
            name: self.name.clone(),
            kind: self.kind,
            names: self.raw.names.clone(),
            aliases: self.raw.aliases.clone(),
            children: self.raw.children.clone(),
            attributes,
            subsets: self.raw.subsets.clone(),
            free: self.raw.free.clone(),
        }
    }

    /// Rebuilds a dimension from its persistent form; validates it like `commit()`.
    pub fn from_data(d: DimensionData) -> crate::Result<Dimension> {
        let n = d.names.len();
        let bad = |what: &str| {
            crate::Error::Storage(format!("dimension '{}' is corrupt: {what}", d.name))
        };
        if d.aliases.len() != n || d.children.len() != n {
            return Err(bad("table sizes differ"));
        }
        let in_range = |id: MemberId| (id as usize) < n && d.names[id as usize].is_some();
        let mut lookup = HashMap::new();
        for (id, name) in d.names.iter().enumerate() {
            let Some(name) = name else { continue };
            for text in std::iter::once(name).chain(&d.aliases[id]) {
                if lookup
                    .insert(text.to_lowercase(), id as MemberId)
                    .is_some_and(|o| o != id as MemberId)
                {
                    return Err(bad(&format!("name '{text}' is not unique")));
                }
            }
            if d.children[id].iter().any(|c| !in_range(c.0)) {
                return Err(bad("edge to an unknown member"));
            }
        }
        let mut attributes: BTreeMap<String, HashMap<MemberId, AttrValue>> = BTreeMap::new();
        for (name, values) in d.attributes {
            if values.iter().any(|v| !in_range(v.0)) {
                return Err(bad("attribute of an unknown member"));
            }
            attributes.insert(name, values.into_iter().collect());
        }
        if d.subsets
            .iter()
            .any(|s| s.members.iter().any(|&m| !in_range(m)))
            || d.free.iter().any(|&f| (f as usize) >= n)
        {
            return Err(bad("reference to an unknown member"));
        }
        let raw = Raw {
            names: d.names,
            aliases: d.aliases,
            children: d.children,
            lookup,
            attributes,
            subsets: d.subsets,
            free: d.free,
        };
        Dimension::build(d.name, d.kind, raw)
    }

    /// Starts editing a copy of this dimension.
    pub fn edit(&self) -> DimensionEditor {
        DimensionEditor::new(self.name.clone(), self.kind, self.raw.clone())
    }

    /// Builds the derived state from raw data; validates hierarchy and weights.
    pub(crate) fn build(name: String, kind: DimensionKind, raw: Raw) -> crate::Result<Dimension> {
        let n = raw.names.len();
        let live = raw.names.iter().flatten().count();

        let mut parents: Vec<Vec<(MemberId, f32)>> = vec![Vec::new(); n];
        for (p, kids) in raw.children.iter().enumerate() {
            for &(c, w) in kids {
                parents[c as usize].push((p as MemberId, w));
            }
        }

        let member_name = |id: usize| raw.names[id].clone().unwrap_or_default();
        let is_live = |id: usize| raw.names[id].is_some();
        let roots: Vec<MemberId> = (0..n)
            .filter(|&i| is_live(i) && parents[i].is_empty())
            .map(|i| i as MemberId)
            .collect();

        // Topological order (parents before children); detects cycles.
        let mut indeg: Vec<u32> = parents.iter().map(|p| p.len() as u32).collect();
        let mut topo: Vec<MemberId> = roots.clone();
        let mut i = 0;
        while i < topo.len() {
            let v = topo[i] as usize;
            i += 1;
            for &(c, _) in &raw.children[v] {
                indeg[c as usize] -= 1;
                if indeg[c as usize] == 0 {
                    topo.push(c);
                }
            }
        }
        if topo.len() != live {
            let stuck = (0..n).find(|&i| is_live(i) && indeg[i] > 0).unwrap_or(0);
            return Err(crate::Error::Cycle {
                dimension: name,
                member: member_name(stuck),
            });
        }

        let mut level = vec![0u8; n];
        for &v in topo.iter().rev() {
            let v = v as usize;
            level[v] = raw.children[v]
                .iter()
                .map(|&(c, _)| level[c as usize].saturating_add(1))
                .max()
                .unwrap_or(0);
        }
        let mut depth = vec![u8::MAX; n];
        for &r in &roots {
            depth[r as usize] = 0;
        }
        for &v in &topo {
            let d = depth[v as usize].saturating_add(1);
            for &(c, _) in &raw.children[v as usize] {
                depth[c as usize] = depth[c as usize].min(d);
            }
        }

        check_path_weights(&name, &raw, &parents)?;

        // DFS numbering of base members along the first hierarchy.
        let mut leaf_ord = vec![NO_ORD; n];
        let mut leaf_members: Vec<MemberId> = Vec::new();
        let mut seen = vec![false; n];
        let mut stack: Vec<(MemberId, usize)> = Vec::new();
        for &r in roots.iter() {
            if seen[r as usize] {
                continue;
            }
            seen[r as usize] = true;
            stack.push((r, 0));
            while let Some(&mut (v, ref mut next)) = stack.last_mut() {
                let kids = &raw.children[v as usize];
                if kids.is_empty() {
                    leaf_ord[v as usize] = leaf_members.len() as u32;
                    leaf_members.push(v);
                    stack.pop();
                } else if *next < kids.len() {
                    let c = kids[*next].0;
                    *next += 1;
                    if !seen[c as usize] {
                        seen[c as usize] = true;
                        stack.push((c, 0));
                    }
                } else {
                    stack.pop();
                }
            }
        }

        let mut search_index: Vec<(String, MemberId)> =
            raw.lookup.iter().map(|(k, &v)| (k.clone(), v)).collect();
        search_index.sort_unstable();

        Ok(Dimension {
            name,
            kind,
            raw,
            parents,
            roots,
            level,
            depth,
            leaf_ord,
            leaf_members,
            search_index,
            leaf_cache: Mutex::new(HashMap::new()),
            live,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind(&self) -> DimensionKind {
        self.kind
    }

    /// Number of members (base + aggregates).
    pub fn len(&self) -> usize {
        self.live
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Number of member slots (live and free); member ids are below this.
    pub fn len_slots(&self) -> usize {
        self.raw.names.len()
    }

    /// Number of base members.
    pub fn n_leaves(&self) -> u32 {
        self.leaf_members.len() as u32
    }

    /// Finds a member by name or alias, ignoring case.
    pub fn resolve(&self, name: &str) -> Option<MemberId> {
        self.raw.lookup.get(&name.to_lowercase()).copied()
    }

    /// Like [`resolve`](Self::resolve) but with a descriptive error.
    pub fn member(&self, name: &str) -> crate::Result<MemberId> {
        self.resolve(name).ok_or_else(|| crate::Error::NotFound {
            kind: "member",
            name: format!("{name}' in dimension '{}", self.name),
        })
    }

    /// Primary name of a member.
    pub fn member_name(&self, id: MemberId) -> Option<&str> {
        self.raw.names.get(id as usize)?.as_deref()
    }

    pub fn aliases(&self, id: MemberId) -> &[String] {
        self.raw.aliases.get(id as usize).map_or(&[], Vec::as_slice)
    }

    pub fn children(&self, id: MemberId) -> &[(MemberId, f32)] {
        self.raw
            .children
            .get(id as usize)
            .map_or(&[], Vec::as_slice)
    }

    pub fn parents(&self, id: MemberId) -> &[(MemberId, f32)] {
        self.parents.get(id as usize).map_or(&[], Vec::as_slice)
    }

    /// Members without a parent, in creation order.
    pub fn roots(&self) -> &[MemberId] {
        &self.roots
    }

    /// Page of children (or of the roots for `None`) for tree navigation.
    pub fn members_page(
        &self,
        parent: Option<MemberId>,
        offset: usize,
        limit: usize,
    ) -> Vec<MemberId> {
        match parent {
            None => self
                .roots
                .iter()
                .copied()
                .skip(offset)
                .take(limit)
                .collect(),
            Some(p) => self
                .children(p)
                .iter()
                .map(|c| c.0)
                .skip(offset)
                .take(limit)
                .collect(),
        }
    }

    pub fn is_leaf(&self, id: MemberId) -> bool {
        self.level.get(id as usize) == Some(&0) && self.raw.names[id as usize].is_some()
    }

    /// 0 for base members, otherwise the height above the base.
    pub fn level(&self, id: MemberId) -> u8 {
        self.level[id as usize]
    }

    /// Shortest distance from a root.
    pub fn depth(&self, id: MemberId) -> u8 {
        self.depth[id as usize]
    }

    /// DFS ordinal of a base member.
    pub fn leaf_ordinal(&self, id: MemberId) -> Option<u32> {
        self.leaf_ord
            .get(id as usize)
            .copied()
            .filter(|&o| o != NO_ORD)
    }

    /// Base member with the given DFS ordinal.
    pub fn leaf_member(&self, ord: u32) -> Option<MemberId> {
        self.leaf_members.get(ord as usize).copied()
    }

    /// Base members below `id` with their path weights (cached). A base member reachable over
    /// several paths counts once; contradicting path weights are rejected at commit.
    pub fn leaf_set(&self, id: MemberId) -> Arc<LeafSet> {
        if let Some(hit) = self.leaf_cache.lock().get(&id) {
            return hit.clone();
        }
        let set = Arc::new(self.compute_leaf_set(id));
        let mut cache = self.leaf_cache.lock();
        if cache.len() >= LEAF_CACHE_CAPACITY {
            cache.clear();
        }
        cache.insert(id, set.clone());
        set
    }

    fn compute_leaf_set(&self, id: MemberId) -> LeafSet {
        let mut leaves: Vec<(u32, f32)> = Vec::new();
        let mut seen = vec![false; self.raw.names.len()];
        let mut stack = vec![(id, 1.0f32)];
        seen[id as usize] = true;
        while let Some((v, w)) = stack.pop() {
            let kids = &self.raw.children[v as usize];
            if kids.is_empty() {
                leaves.push((self.leaf_ord[v as usize], w));
            }
            for &(c, cw) in kids {
                if !seen[c as usize] {
                    seen[c as usize] = true;
                    stack.push((c, w * cw));
                }
            }
        }
        LeafSet::from_leaves(leaves, self.n_leaves())
    }

    /// Attribute value of a member.
    pub fn attribute(&self, attr: &str, id: MemberId) -> Option<&AttrValue> {
        self.raw.attributes.get(attr)?.get(&id)
    }

    pub fn attribute_names(&self) -> impl Iterator<Item = &str> {
        self.raw.attributes.keys().map(String::as_str)
    }

    pub fn subsets(&self) -> &[Subset] {
        &self.raw.subsets
    }

    /// Members whose name or alias starts with / contains `query` (case-insensitive),
    /// prefix matches first. Each member appears once.
    pub fn search(&self, query: &str, limit: usize) -> Vec<MemberId> {
        let q = query.to_lowercase();
        let start = self
            .search_index
            .partition_point(|(k, _)| k.as_str() < q.as_str());
        let mut out: Vec<MemberId> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (k, id) in &self.search_index[start..] {
            if !k.starts_with(&q) || out.len() >= limit {
                break;
            }
            if seen.insert(*id) {
                out.push(*id);
            }
        }
        if out.len() < limit {
            for (k, id) in &self.search_index {
                if out.len() >= limit {
                    break;
                }
                if k.contains(&q) && seen.insert(*id) {
                    out.push(*id);
                }
            }
        }
        out
    }
}

/// Rejects hierarchies in which a base member reaches an aggregate over paths with
/// different weight products (decision E7).
fn check_path_weights(
    name: &str,
    raw: &Raw,
    parents: &[Vec<(MemberId, f32)>],
) -> crate::Result<()> {
    for (w, ps) in parents.iter().enumerate() {
        if ps.len() < 2 {
            continue;
        }
        let mut seen: HashMap<MemberId, f32> = HashMap::from([(w as MemberId, 1.0)]);
        let mut stack = vec![w as MemberId];
        while let Some(v) = stack.pop() {
            let wv = seen[&v];
            for &(p, ew) in &parents[v as usize] {
                let nw = wv * ew;
                match seen.get(&p) {
                    Some(old) if (old - nw).abs() > 1e-6 => {
                        return Err(crate::Error::Conflict(format!(
                            "dimension '{name}': '{}' reaches '{}' over paths with contradicting weights ({old} and {nw})",
                            raw.names[w].as_deref().unwrap_or("?"),
                            raw.names[p as usize].as_deref().unwrap_or("?"),
                        )));
                    }
                    Some(_) => {}
                    None => {
                        seen.insert(p, nw);
                        stack.push(p);
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
