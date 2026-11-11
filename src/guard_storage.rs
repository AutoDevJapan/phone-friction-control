//! The UI policy and guard checkpoint share one process and one storage lock.
//! A worker must re-read the UI-owned policy before publishing its checkpoint.
use crate::app::SavedState;
use crate::enforcement::GuardState;
use crate::persistence::StateStore;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static FILES: Mutex<()> = Mutex::new(());

pub(crate) struct GuardStore {
    dir: PathBuf,
    state: StateStore,
    day: Option<String>,
}

impl GuardStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            state: StateStore::new(dir.join("guard-state.ron")),
            dir,
            day: None,
        }
    }

    pub fn set_day(&mut self, day: Option<String>) {
        self.day = day;
    }

    pub fn is_ready(&self) -> bool {
        self.state.is_ready()
    }

    pub fn load(&mut self) -> io::Result<Option<GuardState>> {
        let _lock = FILES.lock().unwrap_or_else(|p| p.into_inner());
        self.state = StateStore::new(self.dir.join("guard-state.ron"));
        let config = current_config(&self.dir)?;
        remove_legacy_status(&self.dir)?;
        self.state.load_and_migrate_with(|state: &mut GuardState| {
            state.prepare_persistence(&config.policy(), self.day.as_deref());
        })
    }

    pub fn save(&self, state: &GuardState) -> io::Result<()> {
        self.save_locked(state, || {})
    }

    fn save_locked(&self, state: &GuardState, after_policy_read: impl FnOnce()) -> io::Result<()> {
        let _lock = FILES.lock().unwrap_or_else(|p| p.into_inner());
        let config = current_config(&self.dir)?;
        after_policy_read();
        let mut current = state.clone();
        current.prepare_persistence(&config.policy(), self.day.as_deref());
        self.state.save(&current)?;
        remove_legacy_status(&self.dir)
    }
}

pub(crate) fn save_policy(dir: &Path, config: &SavedState, day: Option<&str>) -> io::Result<()> {
    let _lock = FILES.lock().unwrap_or_else(|p| p.into_inner());
    crate::persistence::write_atomic(&dir.join("guard-config.ron"), config)?;
    let mut store = StateStore::new(dir.join("guard-state.ron"));
    store.load_and_migrate_with(|state: &mut GuardState| {
        state.prepare_persistence(&config.policy(), day);
    })?;
    remove_legacy_status(dir)
}

fn current_config(dir: &Path) -> io::Result<SavedState> {
    StateStore::new(dir.join("guard-config.ron"))
        .load()?
        .ok_or_else(|| io::Error::other("Guard policy is unavailable"))
}

fn remove_legacy_status(dir: &Path) -> io::Result<()> {
    // No runtime reader uses this redundant snapshot. Keep live transport in memory only.
    match std::fs::remove_file(dir.join("guard-status.ron")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::TrackedApp;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "phone-guard-storage-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
        fn state(&self) -> GuardState {
            ron::from_str(&std::fs::read_to_string(self.0.join("guard-state.ron")).unwrap())
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn config(packages: &[&str]) -> SavedState {
        let mut config = SavedState::default();
        config.target_apps = packages
            .iter()
            .map(|p| TrackedApp {
                package: (*p).into(),
                label: (*p).into(),
            })
            .collect();
        config
    }
    fn old_state() -> GuardState {
        let mut state = GuardState::default();
        state.sample_usage(
            "2026-11-11",
            1,
            &std::collections::HashMap::from([("video".into(), 61_000), ("mail".into(), 700)]),
        );
        state.cooldown_until_ms = Some(123_456);
        state.last_triggered_day = Some("2026-11-11".into());
        state
    }
    fn seed(fixture: &Fixture) {
        crate::persistence::write_atomic(&fixture.0.join("guard-state.ron"), &old_state()).unwrap();
        std::fs::write(
            fixture.0.join("guard-status.ron"),
            "legacy private snapshot: mail",
        )
        .unwrap();
    }

    #[test]
    fn stopped_service_selection_save_prunes_all_guard_files_and_keeps_break() {
        let fixture = Fixture::new();
        seed(&fixture);
        save_policy(&fixture.0, &config(&["video"]), Some("2026-11-11")).unwrap();
        let state = fixture.state();
        assert!(
            !state.usage_ms.contains_key("mail"),
            "stopped worker left deselected metrics on disk"
        );
        assert_eq!(state.usage_ms["video"], 61_000);
        assert_eq!(state.cooldown_until_ms, Some(123_456));
        assert_eq!(state.last_triggered_day.as_deref(), Some("2026-11-11"));
        assert!(!fixture.0.join("guard-status.ron").exists());
    }

    #[test]
    fn a_stale_worker_cannot_restore_a_newly_deselected_app() {
        let fixture = Fixture::new();
        seed(&fixture);
        save_policy(&fixture.0, &config(&["video", "mail"]), Some("2026-11-11")).unwrap();
        let mut store = GuardStore::new(fixture.0.clone());
        store.set_day(Some("2026-11-11".into()));
        let stale = store.load().unwrap().unwrap();
        save_policy(&fixture.0, &config(&["video"]), Some("2026-11-11")).unwrap();
        store.save(&stale).unwrap();
        assert!(
            !fixture.state().usage_ms.contains_key("mail"),
            "stale worker restored deselected app"
        );
    }

    #[test]
    fn policy_change_serializes_with_an_in_flight_worker_write() {
        let fixture = Fixture::new();
        seed(&fixture);
        save_policy(&fixture.0, &config(&["video", "mail"]), Some("2026-11-11")).unwrap();
        let mut worker = GuardStore::new(fixture.0.clone());
        worker.set_day(Some("2026-11-11".into()));
        let stale = worker.load().unwrap().unwrap();
        let (read_tx, read_rx) = std::sync::mpsc::channel();
        let (continue_tx, continue_rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            worker
                .save_locked(&stale, || {
                    read_tx.send(()).unwrap();
                    continue_rx.recv().unwrap();
                })
                .unwrap();
        });
        read_rx.recv().unwrap();
        // The worker already read A. Queue B while it still owns the actual storage lock.
        let dir = fixture.0.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let editor = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            save_policy(&dir, &config(&["video"]), Some("2026-11-11")).unwrap();
        });
        started_rx.recv().unwrap();
        continue_tx.send(()).unwrap();
        writer.join().unwrap();
        editor.join().unwrap();
        assert!(!fixture.state().usage_ms.contains_key("mail"));
        assert_eq!(fixture.state().usage_ms["video"], 61_000);
    }

    #[test]
    fn a_failed_prune_is_retried_without_losing_current_metrics_or_deadline() {
        let fixture = Fixture::new();
        seed(&fixture);
        let path = fixture.0.join("guard-state.ron");
        let original = std::fs::read(&path).unwrap();
        std::fs::create_dir(path.with_extension("pending")).unwrap();
        assert!(save_policy(&fixture.0, &config(&["video"]), Some("2026-11-11")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir(path.with_extension("pending")).unwrap();
        save_policy(&fixture.0, &config(&["video"]), Some("2026-11-11")).unwrap();
        assert!(!fixture.state().usage_ms.contains_key("mail"));
        assert_eq!(fixture.state().usage_ms["video"], 61_000);
        assert_eq!(fixture.state().cooldown_until_ms, Some(123_456));
    }

    #[test]
    fn unreadable_state_or_policy_never_becomes_a_zero_checkpoint() {
        let fixture = Fixture::new();
        seed(&fixture);
        save_policy(&fixture.0, &config(&["video"]), Some("2026-11-11")).unwrap();
        let path = fixture.0.join("guard-state.ron");
        std::fs::write(&path, "(broken").unwrap();
        assert!(save_policy(&fixture.0, &config(&["video"]), Some("2026-11-11")).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "(broken");
        let mut worker = GuardStore::new(fixture.0.clone());
        assert!(worker.load().is_err());
        assert!(!worker.is_ready());
        assert!(worker.save(&GuardState::default()).is_err());
        crate::persistence::write_atomic(&path, &old_state()).unwrap();
        worker.load().unwrap();
        let current = std::fs::read(&path).unwrap();
        std::fs::write(fixture.0.join("guard-config.ron"), "(broken").unwrap();
        assert!(worker.save(&GuardState::default()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), current);
    }

    #[test]
    fn known_day_prunes_metrics_without_usage_permission_and_unknown_day_keeps_them() {
        let fixture = Fixture::new();
        seed(&fixture);
        save_policy(&fixture.0, &config(&["video"]), None).unwrap();
        assert_eq!(fixture.state().usage_ms["video"], 61_000);
        save_policy(&fixture.0, &config(&["video"]), Some("2026-11-12")).unwrap();
        let current = fixture.state();
        assert!(current.usage_ms.is_empty() && current.playback_ms.is_empty());
        assert_eq!(current.day, "2026-11-12");
        assert_eq!(current.cooldown_until_ms, Some(123_456));
        assert_eq!(current.last_triggered_day.as_deref(), Some("2026-11-11"));
    }
}
