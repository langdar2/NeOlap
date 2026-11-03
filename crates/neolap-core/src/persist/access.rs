//! Access rights: cube access and area grants (plan chapter 6.2).
//!
//! A user's *role* sets what kind of thing they may do (read, plan, model, administer). *Grants*
//! decide on which cubes and where a planner or reader may act: per cube `read`/`write`/`none`,
//! optionally limited to members of one or more dimensions (`regions:'Nord' & version:'Plan'`). A
//! grant on an aggregate covers all its descendants. Without any grant a user has no access to the
//! cube at all, except admins and modelers, who may read and write everything their role allows.
//!
//! **Reading is per cube, writing is per cell.** A user who may read a cube reads all of it,
//! including aggregates. Hiding single elements from readers is not part of v1 (plan chapter 6.2:
//! "Leserechte auf Elementebene sind Phase 2"): it would also have to stop aggregates from
//! revealing hidden values (Total minus the visible part), which needs its own design. So a
//! `none` grant, like a limited `write` grant, restricts writing; it does not hide cells.

use std::sync::Arc;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::{Database, Role};
use crate::cube::{Cube, WritePermission};
use crate::{Error, Result};

/// What a grant allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    None,
    Read,
    Write,
}

impl Access {
    pub fn as_str(self) -> &'static str {
        match self {
            Access::None => "none",
            Access::Read => "read",
            Access::Write => "write",
        }
    }

    pub fn parse(s: &str) -> Result<Access> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Access::None),
            "read" | "lesen" => Ok(Access::Read),
            "write" | "schreiben" => Ok(Access::Write),
            o => Err(Error::Parse(format!(
                "unknown access '{o}' (none, read, write)"
            ))),
        }
    }
}

/// One grant: a user's access to a cube, limited to `area` (dimension → members; empty = whole cube).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub user: String,
    pub cube: String,
    pub access: Access,
    #[serde(default)]
    pub area: Vec<(String, Vec<String>)>,
}

fn encode_area(area: &[(String, Vec<String>)]) -> String {
    serde_json::to_string(area).unwrap_or_default()
}

fn decode_area(text: &str) -> Vec<(String, Vec<String>)> {
    if text.is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(text).unwrap_or_default()
    }
}

/// Allowed base ordinals of one dimension; `None` = all of them.
type AreaSet = Option<Arc<std::collections::HashSet<u32>>>;

/// Grants of one user on one cube, compiled to ordinals for per-cell checks.
pub struct CubeRights {
    /// `true`: the role alone allows everything on this cube (admin, modeler).
    open: bool,
    /// Per grant: access and, per dimension, the allowed base ordinals (`None` = all).
    grants: Vec<(Access, Vec<AreaSet>)>,
    can_read_any: bool,
}

impl CubeRights {
    /// Highest access any grant gives on any cell; `None` if the user has no access at all.
    pub fn best(&self) -> Access {
        if self.open {
            return Access::Write;
        }
        self.grants
            .iter()
            .map(|g| g.0)
            .max()
            .unwrap_or(Access::None)
    }

    pub fn can_read_cube(&self) -> bool {
        self.open || self.can_read_any
    }

    /// Whether the role alone allows everything on the cube (admin, modeler).
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the user may write the base cell (ordinal per dimension).
    pub fn write_permission_for(&self, coords: &[u32]) -> bool {
        self.covers(coords, Access::Write)
    }

    fn covers(&self, coords: &[u32], needed: Access) -> bool {
        if self.open {
            return true;
        }
        // The most specific rule is not modeled; access is the best of all matching grants, but an
        // explicit `none` grant that matches the cell wins (so a block can be carved out of a grant).
        let mut best = Access::None;
        for (access, sets) in &self.grants {
            let hit = sets
                .iter()
                .enumerate()
                .all(|(d, s)| s.as_ref().is_none_or(|s| s.contains(&coords[d])));
            if !hit {
                continue;
            }
            if *access == Access::None {
                return false;
            }
            best = best.max(*access);
        }
        best >= needed
    }

    /// Per-cell write check for [`crate::WriteContext::can_write`].
    pub fn write_permission(self: &Arc<Self>) -> WritePermission {
        let me = self.clone();
        Arc::new(move |coords: &[u32]| me.covers(coords, Access::Write))
    }
}

fn compile_area(cube: &Cube, area: &[(String, Vec<String>)]) -> Result<Vec<AreaSet>> {
    let n = cube.dimensions().len();
    let mut out: Vec<AreaSet> = vec![None; n];
    for (dim, members) in area {
        let d = cube
            .dimensions()
            .iter()
            .position(|x| x.name().eq_ignore_ascii_case(dim))
            .ok_or_else(|| Error::NotFound {
                kind: "dimension",
                name: format!("{dim}' in cube '{}", cube.name()),
            })?;
        let mut set = std::collections::HashSet::new();
        for m in members {
            let id = cube.dimensions()[d].member(m)?;
            // a grant on an aggregate covers all its base members (descendants)
            set.extend(cube.dimensions()[d].leaf_set(id).iter());
        }
        out[d] = Some(Arc::new(set));
    }
    Ok(out)
}

impl Database {
    /// Sets (adds or replaces) a grant. Same user, cube and area replace each other.
    pub fn grant(&mut self, grant: &Grant, by: &str) -> Result<()> {
        let cube = self.cube(&grant.cube).ok_or_else(|| Error::NotFound {
            kind: "cube",
            name: grant.cube.clone(),
        })?;
        compile_area(cube, &grant.area)?; // validates dimensions and members
        let cube_name = cube.name().to_string();
        if !self
            .users()?
            .iter()
            .any(|u| u.name.eq_ignore_ascii_case(&grant.user))
        {
            return Err(Error::NotFound {
                kind: "user",
                name: grant.user.clone(),
            });
        }
        let area = encode_area(&grant.area);
        self.tx(|tx| {
            tx.execute(
                "INSERT INTO grants (user, cube, access, area) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (user, cube, area) DO UPDATE SET access = excluded.access",
                params![grant.user, cube_name, grant.access.as_str(), area],
            )?;
            Self::log(
                tx,
                by,
                "grant",
                &cube_name,
                0,
                0,
                &format!("{}: {} {}", grant.user, grant.access.as_str(), area),
            )
        })
    }

    pub fn revoke(
        &mut self,
        user: &str,
        cube: &str,
        area: &[(String, Vec<String>)],
        by: &str,
    ) -> Result<usize> {
        let cube_name = self
            .cube(cube)
            .map(|c| c.name().to_string())
            .unwrap_or_else(|| cube.to_string());
        let enc = encode_area(area);
        self.tx(|tx| {
            let n = tx.execute(
                "DELETE FROM grants WHERE user = ?1 COLLATE NOCASE AND cube = ?2 AND area = ?3",
                params![user, cube_name, enc],
            )?;
            Self::log(
                tx,
                by,
                "revoke",
                &cube_name,
                0,
                0,
                &format!("{user}: {enc}"),
            )?;
            Ok(n)
        })
    }

    pub fn grants(&self, user: Option<&str>) -> Result<Vec<Grant>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT user, cube, access, area FROM grants WHERE (?1 IS NULL OR user = ?1 COLLATE NOCASE) ORDER BY user, cube, area",
        )?;
        let rows = stmt.query_map(params![user], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|r| {
            let (user, cube, access, area) = r?;
            Ok(Grant {
                user,
                cube,
                access: Access::parse(&access)?,
                area: decode_area(&area),
            })
        })
        .collect()
    }

    /// Rights of a user on a cube. Admins and modelers are not restricted by grants (their role is
    /// the limit); readers and planners need grants; a planner's best access is capped at `Write`,
    /// a reader's at `Read`.
    pub fn rights(&self, user: &str, role: Role, cube: &str) -> Result<Arc<CubeRights>> {
        let c = self.cube(cube).ok_or_else(|| Error::NotFound {
            kind: "cube",
            name: cube.to_string(),
        })?;
        if role >= Role::Modeler {
            return Ok(Arc::new(CubeRights {
                open: true,
                grants: Vec::new(),
                can_read_any: true,
            }));
        }
        let mut grants = Vec::new();
        for g in self
            .grants(Some(user))?
            .into_iter()
            .filter(|g| g.cube.eq_ignore_ascii_case(c.name()))
        {
            let access = if role == Role::Reader {
                g.access.min(Access::Read)
            } else {
                g.access
            };
            grants.push((access, compile_area(c, &g.area)?));
        }
        let can_read_any = grants.iter().any(|g| g.0 >= Access::Read);
        Ok(Arc::new(CubeRights {
            open: false,
            grants,
            can_read_any,
        }))
    }

    /// Cubes a user may read at all.
    pub fn readable_cubes(&self, user: &str, role: Role) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for c in self.cubes() {
            if self.rights(user, role, c.name())?.can_read_cube() {
                out.push(c.name().to_string());
            }
        }
        Ok(out)
    }
}
