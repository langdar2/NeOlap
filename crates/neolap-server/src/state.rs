//! Shared server state: the database, sessions, login rate limit, events and background jobs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use neolap_core::Database;
use neolap_core::persist::Role;
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use tokio::sync::broadcast;

/// A session lives this long without activity.
pub const SESSION_IDLE: Duration = Duration::from_secs(8 * 3600);
/// Failed logins per user name and per client within [`LOGIN_WINDOW`] before they are blocked.
pub const LOGIN_MAX_FAILURES: usize = 5;
pub const LOGIN_WINDOW: Duration = Duration::from_secs(300);
/// Finished jobs are kept this long for their result.
pub const JOB_KEEP: Duration = Duration::from_secs(3600);
/// Unread events a slow SSE client may lag behind before it is dropped.
const EVENT_BACKLOG: usize = 1024;

/// An authenticated user.
#[derive(Debug, Clone, PartialEq)]
pub struct Principal {
    pub name: String,
    pub role: Role,
}

struct Session {
    user: Principal,
    last_seen: Instant,
    /// Random value the client must echo in `X-CSRF-Token` for state-changing requests.
    csrf: String,
}

/// Change notification for open views.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    /// `cells`, `model`, `rules`, `import`
    pub kind: &'static str,
    /// Cube the change belongs to (`None` for model-wide events).
    pub cube: Option<String>,
    pub generation: u64,
    pub user: String,
    pub cells_changed: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Done,
    Failed,
}

/// A background job (import).
#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub id: String,
    pub kind: &'static str,
    pub owner: String,
    pub status: JobStatus,
    /// 0–100 while running.
    pub progress: u8,
    pub phase: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    #[serde(skip)]
    pub finished: Option<Instant>,
}

pub struct AppState {
    pub db: RwLock<Database>,
    pub data_dir: PathBuf,
    sessions: Mutex<HashMap<String, Session>>,
    failures: Mutex<HashMap<String, Vec<Instant>>>,
    pub events: broadcast::Sender<Event>,
    jobs: Mutex<HashMap<String, Job>>,
    /// Allow `Secure`-less cookies (plain HTTP on localhost); set false behind TLS.
    pub insecure_cookies: bool,
    /// Trust `X-Forwarded-For` for the login rate limit (only behind a reverse proxy).
    pub trust_proxy: bool,
}

impl AppState {
    pub fn new(
        db: Database,
        data_dir: PathBuf,
        insecure_cookies: bool,
        trust_proxy: bool,
    ) -> Arc<AppState> {
        let (events, _) = broadcast::channel(EVENT_BACKLOG);
        Arc::new(AppState {
            db: RwLock::new(db),
            data_dir,
            sessions: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
            events,
            jobs: Mutex::new(HashMap::new()),
            insecure_cookies,
            trust_proxy,
        })
    }

    /// Random hex string from the system's secure RNG (32 hex characters per 16 bytes).
    pub fn random_token() -> String {
        // Two Argon2 salts are 32 secure random bytes; no extra RNG dependency is needed.
        let mut out = String::with_capacity(64);
        for _ in 0..2 {
            let part = neolap_core::persist::random_bytes_16();
            out.extend(part.iter().map(|b| format!("{b:02x}")));
        }
        out
    }

    // ---- sessions ----

    /// Starts a session; returns `(session id, csrf token)`.
    pub fn open_session(&self, user: Principal) -> (String, String) {
        let id = Self::random_token();
        let csrf = Self::random_token();
        let mut s = self.sessions.lock();
        s.retain(|_, v| v.last_seen.elapsed() < SESSION_IDLE);
        s.insert(
            id.clone(),
            Session {
                user,
                last_seen: Instant::now(),
                csrf: csrf.clone(),
            },
        );
        (id, csrf)
    }

    /// The user of a session id, refreshing its idle timer.
    pub fn session_user(&self, id: &str) -> Option<(Principal, String)> {
        let mut s = self.sessions.lock();
        match s.get_mut(id) {
            Some(sess) if sess.last_seen.elapsed() < SESSION_IDLE => {
                sess.last_seen = Instant::now();
                Some((sess.user.clone(), sess.csrf.clone()))
            }
            Some(_) => {
                s.remove(id);
                None
            }
            None => None,
        }
    }

    pub fn close_session(&self, id: &str) {
        self.sessions.lock().remove(id);
    }

    /// Ends all sessions of a user (after a password change, role change or removal).
    pub fn close_user_sessions(&self, user: &str) {
        self.sessions
            .lock()
            .retain(|_, v| !v.user.name.eq_ignore_ascii_case(user));
    }

    /// Updates the cached role of a user's sessions after a role change.
    pub fn refresh_role(&self, user: &str, role: Role) {
        for v in self
            .sessions
            .lock()
            .values_mut()
            .filter(|v| v.user.name.eq_ignore_ascii_case(user))
        {
            v.user.role = role;
        }
    }

    // ---- login rate limit ----

    fn recent(list: &mut Vec<Instant>) -> usize {
        list.retain(|t| t.elapsed() < LOGIN_WINDOW);
        list.len()
    }

    /// Whether a login attempt for `user` from `client` is currently allowed.
    pub fn login_allowed(&self, user: &str, client: &str) -> bool {
        let mut f = self.failures.lock();
        [format!("u:{}", user.to_lowercase()), format!("c:{client}")]
            .iter()
            .all(|k| {
                f.get_mut(k)
                    .is_none_or(|l| Self::recent(l) < LOGIN_MAX_FAILURES)
            })
    }

    pub fn login_failed(&self, user: &str, client: &str) {
        let mut f = self.failures.lock();
        for k in [format!("u:{}", user.to_lowercase()), format!("c:{client}")] {
            f.entry(k).or_default().push(Instant::now());
        }
        if f.len() > 10_000 {
            f.retain(|_, l| Self::recent(l) > 0);
        }
    }

    pub fn login_succeeded(&self, user: &str) {
        self.failures
            .lock()
            .remove(&format!("u:{}", user.to_lowercase()));
    }

    // ---- events ----

    pub fn publish(&self, event: Event) {
        // no receiver = nobody is watching; that is fine
        let _ = self.events.send(event);
    }

    // ---- jobs ----

    pub fn new_job(&self, kind: &'static str, owner: &str) -> String {
        let id = Self::random_token()[..24].to_string();
        let mut jobs = self.jobs.lock();
        jobs.retain(|_, j| j.finished.is_none_or(|t| t.elapsed() < JOB_KEEP));
        jobs.insert(
            id.clone(),
            Job {
                id: id.clone(),
                kind,
                owner: owner.to_string(),
                status: JobStatus::Running,
                progress: 0,
                phase: "queued".into(),
                result: None,
                error: None,
                finished: None,
            },
        );
        id
    }

    pub fn update_job(&self, id: &str, f: impl FnOnce(&mut Job)) {
        if let Some(j) = self.jobs.lock().get_mut(id) {
            f(j);
        }
    }

    pub fn job(&self, id: &str) -> Option<Job> {
        self.jobs.lock().get(id).cloned()
    }

    pub fn running_jobs(&self) -> usize {
        self.jobs
            .lock()
            .values()
            .filter(|j| j.status == JobStatus::Running)
            .count()
    }
}
