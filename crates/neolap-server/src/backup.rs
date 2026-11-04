//! Scheduled backups: every `interval` the server writes `backup/auto-<time>.neolap` into the
//! data directory and keeps the newest `keep` of them. Backups made by hand through the API
//! (`backup-<time>.neolap`) are never touched.

use std::{path::Path, sync::Arc, time::Duration};

use crate::state::AppState;

const PREFIX: &str = "auto-";
const SUFFIX: &str = ".neolap";

/// Writes one scheduled backup and deletes the oldest ones beyond `keep`. Returns the file name.
pub fn run_once(state: &AppState, keep: usize) -> Result<String, String> {
    let dir = state.data_dir.join("backup");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = format!(
        "{PREFIX}{}{SUFFIX}",
        neolap_core::persist::timestamp_label()
    );
    state
        .db
        .read()
        .backup(dir.join(&name))
        .map_err(|e| e.to_string())?;
    prune(&dir, keep);
    Ok(name)
}

/// Keeps the newest `keep` scheduled backups (the time stamp in the name sorts chronologically).
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(PREFIX) && n.ends_with(SUFFIX))
        .collect();
    names.sort();
    let excess = names.len().saturating_sub(keep.max(1));
    for old in &names[..excess] {
        if let Err(e) = std::fs::remove_file(dir.join(old)) {
            tracing::warn!("could not delete old backup {old}: {e}");
        }
    }
}

/// Background loop: the first backup after one interval, then every interval. A failed backup is
/// logged and tried again at the next interval.
pub async fn schedule(state: Arc<AppState>, interval: Duration, keep: usize) {
    loop {
        tokio::time::sleep(interval).await;
        let s = state.clone();
        match tokio::task::spawn_blocking(move || run_once(&s, keep)).await {
            Ok(Ok(name)) => tracing::info!("scheduled backup written: backup/{name}"),
            Ok(Err(e)) => tracing::error!("scheduled backup failed: {e}"),
            Err(e) => tracing::error!("scheduled backup failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_the_newest_scheduled_backups_and_leaves_others_alone() {
        let dir = tempfile::tempdir().unwrap();
        for n in [
            "auto-20260101-000000.neolap",
            "auto-20260102-000000.neolap",
            "auto-20260103-000000.neolap",
            "backup-20260101-000000.neolap",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(n), b"x").unwrap();
        }
        prune(dir.path(), 2);
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "auto-20260102-000000.neolap",
                "auto-20260103-000000.neolap",
                "backup-20260101-000000.neolap",
                "notes.txt"
            ]
        );
    }

    #[test]
    fn a_scheduled_backup_opens_as_a_database_with_the_same_users() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = neolap_core::Database::create(dir.path().join("t.neolap")).unwrap();
        db.add_user(
            "ann",
            "a long enough password",
            neolap_core::persist::Role::Admin,
            "t",
        )
        .unwrap();
        let state = AppState::new(db, dir.path().to_path_buf(), true, false);
        let name = run_once(&state, 3).unwrap();
        assert!(
            name.starts_with("auto-") && name.ends_with(".neolap"),
            "{name}"
        );
        let copy = neolap_core::Database::open(dir.path().join("backup").join(&name)).unwrap();
        assert_eq!(copy.users().unwrap().len(), 1);
    }

    /// The loop itself: it writes repeatedly and the retention holds while it runs.
    #[tokio::test]
    async fn the_schedule_writes_repeatedly_and_keeps_only_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let db = neolap_core::Database::create(dir.path().join("t.neolap")).unwrap();
        let state = AppState::new(db, dir.path().to_path_buf(), true, false);
        let task = tokio::spawn(schedule(state, Duration::from_millis(1100), 2));
        // the time stamp has a resolution of one second, so wait for three distinct files
        tokio::time::sleep(Duration::from_millis(3600)).await;
        task.abort();
        let n = std::fs::read_dir(dir.path().join("backup"))
            .unwrap()
            .count();
        assert_eq!(n, 2, "three were written, the oldest was deleted");
    }
}
