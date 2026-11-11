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

    pub fn save(&self, state: &GuardState) -> io::Result<GuardState> {
        self.save_locked(state, || {})
    }

    fn save_locked(
        &self,
        state: &GuardState,
        after_policy_read: impl FnOnce(),
    ) -> io::Result<GuardState> {
        let _lock = FILES.lock().unwrap_or_else(|p| p.into_inner());
        let config = current_config(&self.dir)?;
        after_policy_read();
        let mut current = state.clone();
        // A UsageSnapshot can cross midnight after the loop's calendar read.
        // Its state already carries that observation's day; never rotate it back.
        let observed_day = (!state.day.is_empty()).then_some(state.day.as_str());
        current.prepare_persistence(&config.policy(), observed_day.or(self.day.as_deref()));
        self.state.save(&current)?;
        remove_legacy_status(&self.dir)?;
        Ok(current)
    }
}

#[cfg(test)]
pub(crate) fn save_policy(dir: &Path, config: &SavedState, day: Option<&str>) -> io::Result<()> {
    let _lock = FILES.lock().unwrap_or_else(|p| p.into_inner());
    save_policy_locked(dir, config, day)
}

fn save_policy_locked(dir: &Path, config: &SavedState, day: Option<&str>) -> io::Result<()> {
    let mut current = config.clone();
    if let Some(day) = day {
        current.retain_current_day(day);
    }
    crate::persistence::write_atomic(&dir.join("guard-config.ron"), &current)?;
    let mut store = StateStore::new(dir.join("guard-state.ron"));
    store.load_and_migrate_with(|state: &mut GuardState| {
        state.prepare_persistence(&current.policy(), day);
    })?;
    remove_legacy_status(dir)
}

pub(crate) fn load_usage(
    store: &mut StateStore,
    day: Option<&str>,
) -> io::Result<Option<SavedState>> {
    let _lock = FILES.lock().unwrap_or_else(|p| p.into_inner());
    store.load_and_migrate_with(|state: &mut SavedState| {
        if let Some(day) = day {
            state.retain_current_day(day);
        }
    })
}

pub(crate) fn save_usage(
    store: &StateStore,
    dir: &Path,
    state: &SavedState,
    day: Option<&str>,
) -> io::Result<()> {
    let _lock = FILES.lock().unwrap_or_else(|p| p.into_inner());
    let mut current = state.clone();
    if let Some(day) = day {
        current.retain_current_day(day);
    }
    store.save(&current)?;
    save_policy_locked(dir, &current, day)
}

pub(crate) fn sync_calendar(dir: &Path, day: Option<&str>) -> io::Result<()> {
    let Some(day) = day.filter(|day| !day.is_empty()) else {
        return Ok(());
    };
    let _lock = FILES.lock().unwrap_or_else(|p| p.into_inner());
    for file in ["usage-state.ron", "guard-config.ron"] {
        let mut store = StateStore::new(dir.join(file));
        store.load_and_migrate_with(|state: &mut SavedState| state.retain_current_day(day))?;
    }
    Ok(())
}

fn current_config(dir: &Path) -> io::Result<SavedState> {
    StateStore::new(dir.join("guard-config.ron"))
        .load()?
        .ok_or_else(|| io::Error::other("Guard policy is unavailable"))
}

fn remove_legacy_status(dir: &Path) -> io::Result<()> {
    // No runtime reader uses this redundant snapshot. Keep live transport in memory only.
    for name in ["guard-status.ron", "guard-status.pending"] {
        match std::fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
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
        std::fs::write(
            fixture.0.join("guard-status.pending"),
            "old interrupted snapshot: mail",
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
        assert!(!fixture.0.join("guard-status.pending").exists());
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

    #[test]
    fn midnight_snapshot_is_not_rewritten_to_the_workers_previous_day() {
        let fixture = Fixture::new();
        seed(&fixture);
        save_policy(&fixture.0, &config(&["video"]), Some("2026-11-11")).unwrap();
        let mut worker = GuardStore::new(fixture.0.clone());
        worker.set_day(Some("2026-11-11".into()));
        let mut state = worker.load().unwrap().unwrap();
        state.sample_usage(
            "2026-11-12",
            1,
            &std::collections::HashMap::from([("video".into(), 99)]),
        );
        worker.save(&state).unwrap();
        let disk = fixture.state();
        assert_eq!(disk.day, "2026-11-12");
        assert_eq!(disk.usage_ms["video"], 99);
    }

    fn dated_config() -> SavedState {
        ron::from_str("(target_apps:[(label:\"Video\",package:\"video\")],current_day_key:\"2026-11-11\",daily_records:[(day_key:\"2026-11-11\",label:\"Today\",used_ms:2640000,playback_only:false)],cooldown_until_ms:Some(123456),last_limit_triggered_day:Some(\"2026-11-11\"))").unwrap()
    }

    #[test]
    fn a_known_save_day_normalizes_the_policy_record_itself() {
        let fixture = Fixture::new();
        seed(&fixture);
        save_policy(&fixture.0, &dated_config(), Some("2026-11-12")).unwrap();
        let text = std::fs::read_to_string(fixture.0.join("guard-config.ron")).unwrap();
        assert!(text.contains("current_day_key:\"2026-11-12\""));
        assert!(text.contains("daily_records:[]"));
        assert!(text.contains("cooldown_until_ms:Some(123456)"));
    }

    #[test]
    fn background_calendar_prunes_both_ui_copies_and_suspend_save_cannot_restore_them() {
        let fixture = Fixture::new();
        seed(&fixture);
        let stale = dated_config();
        crate::persistence::write_atomic(&fixture.0.join("usage-state.ron"), &stale).unwrap();
        save_policy(&fixture.0, &stale, Some("2026-11-11")).unwrap();
        let mut ui = StateStore::new(fixture.0.join("usage-state.ron"));
        let memory = load_usage(&mut ui, Some("2026-11-11")).unwrap().unwrap();
        sync_calendar(&fixture.0, Some("2026-11-12")).unwrap();
        for file in ["usage-state.ron", "guard-config.ron"] {
            let text = std::fs::read_to_string(fixture.0.join(file)).unwrap();
            assert!(text.contains("current_day_key:\"2026-11-12\""));
            assert!(text.contains("daily_records:[]"));
            assert!(text.contains("cooldown_until_ms:Some(123456)"));
        }
        // This is the actual save helper used by App::save without a prior UI/logic pass.
        save_usage(&ui, &fixture.0, &memory, Some("2026-11-12")).unwrap();
        for file in ["usage-state.ron", "guard-config.ron"] {
            let text = std::fs::read_to_string(fixture.0.join(file)).unwrap();
            assert!(text.contains("current_day_key:\"2026-11-12\""));
            assert!(!text.contains("2640000"));
        }
        assert_eq!(fixture.state().cooldown_until_ms, Some(123_456));
    }

    #[test]
    fn rollback_is_a_current_day_change_and_keeps_the_completed_marker_across_restart() {
        let fixture = Fixture::new();
        seed(&fixture);
        let config = dated_config();
        crate::persistence::write_atomic(&fixture.0.join("usage-state.ron"), &config).unwrap();
        save_policy(&fixture.0, &config, Some("2026-11-11")).unwrap();
        sync_calendar(&fixture.0, Some("2026-11-12")).unwrap();
        sync_calendar(&fixture.0, Some("2026-11-10")).unwrap();
        let mut ui = StateStore::new(fixture.0.join("usage-state.ron"));
        let restored = load_usage(&mut ui, Some("2026-11-10")).unwrap().unwrap();
        assert_eq!(restored.current_day_key, "2026-11-10");
        assert_eq!(
            restored.last_limit_triggered_day.as_deref(),
            Some("2026-11-11")
        );
        assert_eq!(restored.cooldown_until_ms, Some(123_456));
        let mut worker = GuardStore::new(fixture.0.clone());
        worker.set_day(Some("2026-11-10".into()));
        let mut state = worker.load().unwrap().unwrap();
        assert_eq!(state.day, "2026-11-10");
        state.sample_usage(
            "2026-11-10",
            2,
            &std::collections::HashMap::from([("video".into(), 2_000_000)]),
        );
        let deadline = state.cooldown_until_ms;
        state.enforce(&restored.policy(), 1_000);
        assert_eq!(state.cooldown_until_ms, deadline);
        worker.save(&state).unwrap();
        let mut restarted = GuardStore::new(fixture.0.clone());
        restarted.set_day(Some("2026-11-10".into()));
        assert_eq!(restarted.load().unwrap().unwrap(), fixture.state());
    }

    #[test]
    fn failed_background_copy_migration_preserves_bytes_then_retries() {
        let fixture = Fixture::new();
        let stale = dated_config();
        let path = fixture.0.join("usage-state.ron");
        crate::persistence::write_atomic(&path, &stale).unwrap();
        let original = std::fs::read(&path).unwrap();
        std::fs::create_dir(path.with_extension("pending")).unwrap();
        assert!(sync_calendar(&fixture.0, Some("2026-11-12")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir(path.with_extension("pending")).unwrap();
        sync_calendar(&fixture.0, Some("2026-11-12")).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("daily_records:[]")
        );
    }
}
