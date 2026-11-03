//! Change set for a [`Dimension`].

use super::{AttrValue, Dimension, DimensionKind, Raw, Subset};
use crate::{Error, MemberId, Result};

/// Collects changes to a dimension; nothing becomes visible before [`commit`](Self::commit).
pub struct DimensionEditor {
    name: String,
    kind: DimensionKind,
    raw: Raw,
    /// Slots freed in this session; reusable only after `commit()`, so that a member id always
    /// names the same member between two committed states (facts are keyed by it).
    removed: Vec<MemberId>,
}

fn key(name: &str) -> String {
    name.to_lowercase()
}

impl DimensionEditor {
    pub(crate) fn new(name: String, kind: DimensionKind, raw: Raw) -> Self {
        DimensionEditor {
            name,
            kind,
            raw,
            removed: Vec::new(),
        }
    }

    fn check_name(name: &str) -> Result<()> {
        if name.trim().is_empty() {
            return Err(Error::InvalidOperation(
                "member name must not be empty".into(),
            ));
        }
        Ok(())
    }

    fn slot(&self, id: MemberId) -> Result<usize> {
        match self.raw.names.get(id as usize) {
            Some(Some(_)) => Ok(id as usize),
            _ => Err(Error::NotFound {
                kind: "member id",
                name: id.to_string(),
            }),
        }
    }

    /// Id of a member by name or alias.
    pub fn resolve(&self, name: &str) -> Option<MemberId> {
        self.raw.lookup.get(&key(name)).copied()
    }

    /// Adds a member (initially a root and base member). Fails if the name or alias exists.
    pub fn add(&mut self, name: &str) -> Result<MemberId> {
        Self::check_name(name)?;
        let k = key(name);
        if self.raw.lookup.contains_key(&k) {
            return Err(Error::Conflict(format!(
                "name '{name}' already exists in dimension '{}'",
                self.name
            )));
        }
        let id = match self.raw.free.pop() {
            Some(id) => {
                self.raw.names[id as usize] = Some(name.to_string());
                id
            }
            None => {
                self.raw.names.push(Some(name.to_string()));
                self.raw.aliases.push(Vec::new());
                self.raw.children.push(Vec::new());
                (self.raw.names.len() - 1) as MemberId
            }
        };
        self.raw.lookup.insert(k, id);
        Ok(id)
    }

    /// Returns the existing member or adds it.
    pub fn get_or_add(&mut self, name: &str) -> Result<MemberId> {
        match self.resolve(name) {
            Some(id) => Ok(id),
            None => self.add(name),
        }
    }

    /// Makes `child` a child of `parent` with the given weight (updates the weight if the edge exists).
    pub fn add_edge(&mut self, parent: MemberId, child: MemberId, weight: f32) -> Result<()> {
        if !weight.is_finite() {
            return Err(Error::InvalidOperation(
                "edge weight must be a finite number".into(),
            ));
        }
        let p = self.slot(parent)?;
        self.slot(child)?;
        if parent == child {
            return Err(Error::Cycle {
                dimension: self.name.clone(),
                member: self.raw.names[p].clone().unwrap(),
            });
        }
        match self.raw.children[p].iter_mut().find(|e| e.0 == child) {
            Some(e) => e.1 = weight,
            None => self.raw.children[p].push((child, weight)),
        }
        Ok(())
    }

    /// Adds (or reuses) the named children below `parent`, all with weight 1.
    pub fn add_children(&mut self, parent: MemberId, children: &[&str]) -> Result<Vec<MemberId>> {
        children
            .iter()
            .map(|c| {
                let id = self.get_or_add(c)?;
                self.add_edge(parent, id, 1.0)?;
                Ok(id)
            })
            .collect()
    }

    /// Renames a member; the old name is freed.
    pub fn rename(&mut self, id: MemberId, new_name: &str) -> Result<()> {
        Self::check_name(new_name)?;
        let i = self.slot(id)?;
        let new_key = key(new_name);
        if let Some(&other) = self.raw.lookup.get(&new_key) {
            if other != id {
                return Err(Error::Conflict(format!(
                    "name '{new_name}' already exists in dimension '{}'",
                    self.name
                )));
            }
            if self.raw.aliases[i].iter().any(|a| key(a) == new_key) {
                return Err(Error::Conflict(format!(
                    "'{new_name}' is already an alias of this member"
                )));
            }
        }
        let old = self.raw.names[i].replace(new_name.to_string()).unwrap();
        self.raw.lookup.remove(&key(&old));
        self.raw.lookup.insert(new_key, id);
        Ok(())
    }

    /// Removes a member. Its children stay (and become roots unless they have other parents).
    pub fn remove(&mut self, id: MemberId) -> Result<()> {
        let i = self.slot(id)?;
        let name = self.raw.names[i].take().unwrap();
        self.raw.lookup.remove(&key(&name));
        for a in std::mem::take(&mut self.raw.aliases[i]) {
            self.raw.lookup.remove(&key(&a));
        }
        self.raw.children[i].clear();
        for kids in &mut self.raw.children {
            kids.retain(|e| e.0 != id);
        }
        for attr in self.raw.attributes.values_mut() {
            attr.remove(&id);
        }
        for s in &mut self.raw.subsets {
            s.members.retain(|&m| m != id);
        }
        self.removed.push(id);
        Ok(())
    }

    /// Moves a member from `from` (or from all parents for `None`) to `to` (`None` = root),
    /// keeping the weight of the removed edge if there is exactly one.
    pub fn move_member(
        &mut self,
        id: MemberId,
        from: Option<MemberId>,
        to: Option<MemberId>,
    ) -> Result<()> {
        self.slot(id)?;
        let mut weight = 1.0;
        for (p, kids) in self.raw.children.iter_mut().enumerate() {
            if from.is_none_or(|f| f as usize == p)
                && let Some(pos) = kids.iter().position(|e| e.0 == id)
            {
                weight = kids.remove(pos).1;
            }
        }
        if let Some(t) = to {
            self.add_edge(t, id, weight)?;
        }
        Ok(())
    }

    /// Sets the weight of an existing edge.
    pub fn set_weight(&mut self, parent: MemberId, child: MemberId, weight: f32) -> Result<()> {
        if !weight.is_finite() {
            return Err(Error::InvalidOperation(
                "edge weight must be a finite number".into(),
            ));
        }
        let p = self.slot(parent)?;
        match self.raw.children[p].iter_mut().find(|e| e.0 == child) {
            Some(e) => {
                e.1 = weight;
                Ok(())
            }
            None => Err(Error::NotFound {
                kind: "edge",
                name: format!("{parent} -> {child}"),
            }),
        }
    }

    /// Adds an alias; aliases share the namespace of member names.
    pub fn set_alias(&mut self, id: MemberId, alias: &str) -> Result<()> {
        Self::check_name(alias)?;
        let i = self.slot(id)?;
        let k = key(alias);
        match self.raw.lookup.get(&k) {
            Some(&o) if o == id => return Ok(()),
            Some(_) => {
                return Err(Error::Conflict(format!(
                    "alias '{alias}' collides with an existing name or alias"
                )));
            }
            None => {}
        }
        self.raw.aliases[i].push(alias.to_string());
        self.raw.lookup.insert(k, id);
        Ok(())
    }

    pub fn set_attribute(&mut self, attr: &str, id: MemberId, value: AttrValue) -> Result<()> {
        self.slot(id)?;
        self.raw
            .attributes
            .entry(attr.to_string())
            .or_default()
            .insert(id, value);
        Ok(())
    }

    /// Creates or replaces a subset.
    pub fn set_subset(&mut self, name: &str, members: Vec<MemberId>) -> Result<()> {
        for &m in &members {
            self.slot(m)?;
        }
        match self
            .raw
            .subsets
            .iter_mut()
            .find(|s| s.name.eq_ignore_ascii_case(name))
        {
            Some(s) => s.members = members,
            None => self.raw.subsets.push(Subset {
                name: name.to_string(),
                members,
            }),
        }
        Ok(())
    }

    /// Drops all edges, aliases, attributes and subsets but keeps the members and their ids. A
    /// following import then rebuilds the structure from a file without renumbering anything.
    pub fn reset_structure(&mut self) {
        for kids in &mut self.raw.children {
            kids.clear();
        }
        for (i, aliases) in self.raw.aliases.iter_mut().enumerate() {
            for a in aliases.drain(..) {
                self.raw.lookup.remove(&key(&a));
            }
            debug_assert!(self.raw.names.get(i).is_some() || aliases.is_empty());
        }
        self.raw.attributes.clear();
        self.raw.subsets.clear();
    }

    pub fn set_kind(&mut self, kind: DimensionKind) {
        self.kind = kind;
    }

    /// Ids of all members.
    pub fn member_ids(&self) -> Vec<MemberId> {
        self.live_ids()
    }

    pub fn len(&self) -> usize {
        self.raw.names.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Validates (cycles, contradicting path weights) and builds the new dimension.
    pub fn commit(mut self) -> Result<Dimension> {
        self.raw.free.extend(self.removed);
        Dimension::build(self.name, self.kind, self.raw)
    }
}

impl DimensionEditor {
    pub fn name_of_member(&self, id: MemberId) -> String {
        self.name_of(id)
    }

    pub(crate) fn name_of(&self, id: MemberId) -> String {
        self.raw.names[id as usize].clone().unwrap_or_default()
    }

    pub(crate) fn live_ids(&self) -> Vec<MemberId> {
        (0..self.raw.names.len())
            .filter(|&i| self.raw.names[i].is_some())
            .map(|i| i as MemberId)
            .collect()
    }
}
