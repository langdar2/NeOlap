//! Users, roles and API tokens (stored in the database file).
//!
//! Passwords are stored as Argon2id hashes (PHC string). An API token has the form
//! `nlp_<id>_<secret>`: it is shown once, the file only holds an Argon2 hash of the secret. The
//! public `<id>` makes the lookup a primary-key access; the hash does the verification.

use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Argon2, PasswordHash};
use rusqlite::{OptionalExtension, params};

use super::Database;
use crate::{Error, Result};

/// Role of a user (plan chapter 6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Reads only.
    Reader,
    /// Reads, writes within granted areas, runs imports.
    Planner,
    /// Changes dimensions, cubes, rules and import specifications.
    Modeler,
    /// Everything, including users and backups.
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Reader => "reader",
            Role::Planner => "planner",
            Role::Modeler => "modeler",
            Role::Admin => "admin",
        }
    }

    pub fn parse(s: &str) -> Result<Role> {
        match s.trim().to_ascii_lowercase().as_str() {
            "reader" | "leser" => Ok(Role::Reader),
            "planner" | "planer" => Ok(Role::Planner),
            "modeler" | "modellierer" => Ok(Role::Modeler),
            "admin" => Ok(Role::Admin),
            other => Err(Error::Parse(format!(
                "unknown role '{other}' (reader, planner, modeler, admin)"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct User {
    pub name: String,
    pub role: Role,
}

/// Shortest accepted password.
pub const MIN_PASSWORD_LEN: usize = 10;

fn hash(secret: &str) -> Result<String> {
    Argon2::default()
        .hash_password(secret.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| Error::Storage(format!("password hashing failed: {e}")))
}

fn verify(secret: &str, stored: &str) -> bool {
    PasswordHash::new(stored).is_ok_and(|h| {
        Argon2::default()
            .verify_password(secret.as_bytes(), &h)
            .is_ok()
    })
}

fn check_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || "._-@".contains(c))
    {
        return Err(Error::InvalidOperation(
            "a user name has 1–64 characters: letters, digits and . _ - @".into(),
        ));
    }
    Ok(())
}

impl Database {
    /// Adds a user. Fails if the name exists (case-insensitive) or the password is too short.
    pub fn add_user(&mut self, name: &str, password: &str, role: Role, by: &str) -> Result<()> {
        check_name(name)?;
        if password.chars().count() < MIN_PASSWORD_LEN {
            return Err(Error::InvalidOperation(format!(
                "the password needs at least {MIN_PASSWORD_LEN} characters"
            )));
        }
        let h = hash(password)?;
        self.tx(|tx| {
            let exists: Option<String> = tx
                .query_row(
                    "SELECT name FROM users WHERE name = ?1 COLLATE NOCASE",
                    params![name],
                    |r| r.get(0),
                )
                .optional()?;
            if exists.is_some() {
                return Err(Error::Conflict(format!("user '{name}' already exists")));
            }
            tx.execute(
                "INSERT INTO users (name, password_hash, role) VALUES (?1, ?2, ?3)",
                params![name, h, role.as_str()],
            )?;
            Self::log(
                tx,
                by,
                "user_add",
                "",
                0,
                0,
                &format!("{name} ({})", role.as_str()),
            )
        })
    }

    pub fn users(&self) -> Result<Vec<User>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT name, role FROM users ORDER BY name COLLATE NOCASE")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.map(|r| {
            let (name, role) = r?;
            Ok(User {
                name,
                role: Role::parse(&role)?,
            })
        })
        .collect()
    }

    /// Checks name and password; returns the user. The same error for unknown user and wrong
    /// password, and a hash is always computed, so neither the message nor the time tells which.
    pub fn authenticate(&self, name: &str, password: &str) -> Result<User> {
        let found: Option<(String, String, String)> = self
            .conn
            .lock()
            .query_row(
                "SELECT name, password_hash, role FROM users WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let dummy = DUMMY_HASH.get_or_init(|| hash("not-a-real-password").unwrap_or_default());
        let (stored, ok_user) = match &found {
            Some((_, h, _)) => (h.as_str(), true),
            None => (dummy.as_str(), false),
        };
        let ok_pw = verify(password, stored);
        match (found, ok_user && ok_pw) {
            (Some((name, _, role)), true) => Ok(User {
                name,
                role: Role::parse(&role)?,
            }),
            _ => Err(Error::Denied("wrong user name or password".into())),
        }
    }

    pub fn set_role(&mut self, name: &str, role: Role, by: &str) -> Result<()> {
        self.ensure_other_admin(name, role)?;
        self.tx(|tx| {
            let n = tx.execute(
                "UPDATE users SET role = ?2 WHERE name = ?1 COLLATE NOCASE",
                params![name, role.as_str()],
            )?;
            if n == 0 {
                return Err(Error::NotFound {
                    kind: "user",
                    name: name.to_string(),
                });
            }
            Self::log(
                tx,
                by,
                "user_role",
                "",
                0,
                0,
                &format!("{name} → {}", role.as_str()),
            )
        })
    }

    pub fn set_password(&mut self, name: &str, password: &str, by: &str) -> Result<()> {
        if password.chars().count() < MIN_PASSWORD_LEN {
            return Err(Error::InvalidOperation(format!(
                "the password needs at least {MIN_PASSWORD_LEN} characters"
            )));
        }
        let h = hash(password)?;
        self.tx(|tx| {
            let n = tx.execute(
                "UPDATE users SET password_hash = ?2 WHERE name = ?1 COLLATE NOCASE",
                params![name, h],
            )?;
            if n == 0 {
                return Err(Error::NotFound {
                    kind: "user",
                    name: name.to_string(),
                });
            }
            // a new password invalidates the user's tokens
            tx.execute(
                "DELETE FROM tokens WHERE user = ?1 COLLATE NOCASE",
                params![name],
            )?;
            Self::log(tx, by, "user_password", "", 0, 0, name)
        })
    }

    pub fn remove_user(&mut self, name: &str, by: &str) -> Result<()> {
        self.ensure_other_admin(name, Role::Reader)?;
        self.tx(|tx| {
            tx.execute(
                "DELETE FROM tokens WHERE user = ?1 COLLATE NOCASE",
                params![name],
            )?;
            let n = tx.execute(
                "DELETE FROM users WHERE name = ?1 COLLATE NOCASE",
                params![name],
            )?;
            if n == 0 {
                return Err(Error::NotFound {
                    kind: "user",
                    name: name.to_string(),
                });
            }
            Self::log(tx, by, "user_remove", "", 0, 0, name)
        })
    }

    /// The last admin can be neither removed nor demoted: nobody could manage users afterwards.
    fn ensure_other_admin(&self, name: &str, new_role: Role) -> Result<()> {
        if new_role == Role::Admin {
            return Ok(());
        }
        let users = self.users()?;
        let is_admin = users
            .iter()
            .any(|u| u.name.eq_ignore_ascii_case(name) && u.role == Role::Admin);
        let others = users
            .iter()
            .filter(|u| u.role == Role::Admin && !u.name.eq_ignore_ascii_case(name))
            .count();
        if is_admin && others == 0 {
            return Err(Error::InvalidOperation(
                "this is the last admin; create another admin first".into(),
            ));
        }
        Ok(())
    }

    /// Creates an API token for the user. The secret is returned once and cannot be read again.
    pub fn create_token(&mut self, user: &str, label: &str, by: &str) -> Result<String> {
        let exists = self
            .users()?
            .into_iter()
            .find(|u| u.name.eq_ignore_ascii_case(user));
        let Some(u) = exists else {
            return Err(Error::NotFound {
                kind: "user",
                name: user.to_string(),
            });
        };
        let id = random_hex(6)?;
        let secret = random_hex(24)?;
        let h = hash(&secret)?;
        self.tx(|tx| {
            tx.execute(
                "INSERT INTO tokens (id, user, label, token_hash, created) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, u.name, label, h, super::now()],
            )?;
            Self::log(tx, by, "token_create", "", 0, 0, &format!("{} ({label})", u.name))
        })?;
        Ok(format!("nlp_{id}_{secret}"))
    }

    /// Resolves a token (`nlp_<id>_<secret>`) to its user.
    pub fn authenticate_token(&self, token: &str) -> Result<User> {
        let denied = || Error::Denied("invalid token".into());
        let rest = token.strip_prefix("nlp_").ok_or_else(denied)?;
        let (id, secret) = rest.split_once('_').ok_or_else(denied)?;
        let found: Option<(String, String)> = self
            .conn
            .lock()
            .query_row(
                "SELECT user, token_hash FROM tokens WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let dummy = DUMMY_HASH.get_or_init(|| hash("not-a-real-password").unwrap_or_default());
        let stored = found.as_ref().map_or(dummy.as_str(), |f| f.1.as_str());
        let ok = verify(secret, stored);
        match found {
            Some((user, _)) if ok => self
                .users()?
                .into_iter()
                .find(|u| u.name == user)
                .ok_or_else(denied),
            _ => Err(denied()),
        }
    }

    pub fn revoke_tokens(&mut self, user: &str, by: &str) -> Result<usize> {
        self.tx(|tx| {
            let n = tx.execute(
                "DELETE FROM tokens WHERE user = ?1 COLLATE NOCASE",
                params![user],
            )?;
            Self::log(tx, by, "token_revoke", "", 0, 0, user)?;
            Ok(n)
        })
    }
}

static DUMMY_HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// 16 random bytes from the system's secure RNG (for session ids and CSRF tokens in the server).
///
/// # Panics
/// If the operating system has no secure random source, which cannot be recovered from.
pub fn random_bytes_16() -> [u8; 16] {
    argon2::password_hash::try_generate_salt().expect("the system has no secure random source")
}

/// `bytes` (at most 32) random bytes from the system's secure RNG as hex, on every platform.
fn random_hex(bytes: usize) -> Result<String> {
    use argon2::password_hash::try_generate_salt;
    let mut pool = Vec::with_capacity(32);
    for _ in 0..2 {
        let part = try_generate_salt()
            .map_err(|e| Error::Storage(format!("no secure random source: {e}")))?;
        pool.extend(part);
    }
    Ok(pool
        .iter()
        .take(bytes.min(32))
        .map(|b| format!("{b:02x}"))
        .collect())
}
