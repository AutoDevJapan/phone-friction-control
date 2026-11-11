//! All Android state owners use one lock, then sample the clock, merge and persist.
//! A failed clock read performs no file operation; a stale caller cannot choose a day.
use crate::app::SavedState;
use crate::enforcement::GuardState;
use crate::persistence::StateStore;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static FILES: Mutex<()> = Mutex::new(());
pub(crate) type Calendar = (String, i64);

fn transaction<T>(
    calendar: impl FnOnce() -> io::Result<Calendar>,
    run: impl FnOnce(&str, i64) -> io::Result<T>,
) -> io::Result<T> {
    let _lock = FILES.lock().unwrap_or_else(|p| p.into_inner());
    let (day, now) = calendar()?;
    if day.is_empty() {
        return Err(io::Error::other("Calendar is unavailable"));
    }
    run(&day, now)
}

fn read<T: serde::de::DeserializeOwned>(path: PathBuf) -> io::Result<Option<T>> {
    StateStore::new(path).load()
}

trait Stored: serde::Serialize + serde::de::DeserializeOwned + PartialEq {
    fn matches_projection(&self, current: &Self) -> bool {
        self == current
    }
}
impl Stored for SavedState {}
impl Stored for GuardState {
    fn matches_projection(&self, current: &Self) -> bool {
        self.stored_matches(current)
    }
}
#[cfg(test)]
impl Stored for std::collections::HashMap<String, u64> {}

fn put<T: Stored>(path: PathBuf, value: &T) -> io::Result<()> {
    match std::fs::read_to_string(&path) {
        Ok(old) if ron::from_str::<T>(&old).is_ok_and(|old| old.matches_projection(value)) => {
            match std::fs::remove_file(path.with_extension("pending")) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            }
        }
        Ok(_) => crate::persistence::write_atomic(&path, value),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            crate::persistence::write_atomic(&path, value)
        }
        Err(e) => Err(e),
    }
}

fn normalize_guard(state: &mut GuardState, config: &SavedState, day: &str, now: i64) {
    state.prepare_persistence(&config.policy(), Some(day));
    state.cooldown_until_ms = state.cooldown_until_ms.filter(|until| *until > now);
    state.import_break(
        config.cooldown_until_ms,
        &config.limit_triggered_day,
        &config.last_limit_triggered_day,
        now,
    );
}

pub(crate) struct GuardStore {
    dir: PathBuf,
    state: StateStore,
    #[cfg(test)]
    day: Option<String>,
}

impl GuardStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            state: StateStore::new(dir.join("guard-state.ron")),
            dir,
            #[cfg(test)]
            day: None,
        }
    }
    pub fn is_ready(&self) -> bool {
        self.state.is_ready()
    }

    pub fn load_transaction(
        &mut self,
        calendar: impl FnOnce() -> io::Result<Calendar>,
    ) -> io::Result<Option<GuardState>> {
        self.state = StateStore::new(self.dir.join("guard-state.ron"));
        transaction(calendar, |day, now| {
            let mut config = read::<SavedState>(self.dir.join("guard-config.ron"))?
                .ok_or_else(|| io::Error::other("Guard policy unavailable"))?;
            if let Some(usage) = read::<SavedState>(self.dir.join("usage-state.ron"))? {
                config.merge_persisted(&usage, day, now);
            }
            // Complete fallible legacy cleanup before StateStore can become ready.
            remove_legacy_status(&self.dir)?;
            let result = self.state.load_and_migrate_with(|state: &mut GuardState| {
                normalize_guard(state, &config, day, now)
            })?;
            Ok(result)
        })
    }

    pub fn save_transaction(
        &self,
        state: &GuardState,
        calendar: impl FnOnce() -> io::Result<Calendar>,
    ) -> io::Result<GuardState> {
        self.save_transaction_hook(state, calendar, || {})
    }

    fn save_transaction_hook(
        &self,
        state: &GuardState,
        calendar: impl FnOnce() -> io::Result<Calendar>,
        after_read: impl FnOnce(),
    ) -> io::Result<GuardState> {
        transaction(calendar, |day, now| {
            if !self.state.is_ready() {
                return Err(io::Error::other("Guard state has not been read"));
            }
            let mut config = read::<SavedState>(self.dir.join("guard-config.ron"))?
                .ok_or_else(|| io::Error::other("Guard policy unavailable"))?;
            if let Some(usage) = read::<SavedState>(self.dir.join("usage-state.ron"))? {
                config.merge_persisted(&usage, day, now);
            }
            let disk = read::<GuardState>(self.dir.join("guard-state.ron"))?;
            after_read();
            let mut current = state.clone();
            normalize_guard(&mut current, &config, day, now);
            if let Some(mut disk) = disk {
                normalize_guard(&mut disk, &config, day, now);
                current.merge_checkpoint(&disk, now);
            }
            current.cooldown_until_ms = current.cooldown_until_ms.filter(|until| *until > now);
            put(self.dir.join("guard-state.ron"), &current)?;
            remove_legacy_status(&self.dir)?;
            Ok(current)
        })
    }

    #[cfg(test)]
    pub fn set_day(&mut self, day: Option<String>) {
        self.day = day;
    }
    #[cfg(test)]
    pub fn load(&mut self) -> io::Result<Option<GuardState>> {
        let day = self.day.clone().unwrap_or_else(|| "2026-11-11".into());
        self.load_transaction(|| Ok((day, 1000)))
    }
    #[cfg(test)]
    pub fn save(&self, state: &GuardState) -> io::Result<GuardState> {
        let day = if state.day.is_empty() {
            self.day.clone().unwrap_or_else(|| "2026-11-11".into())
        } else {
            state.day.clone()
        };
        self.save_transaction(state, || Ok((day, 1000)))
    }
    #[cfg(test)]
    fn save_locked(&self, state: &GuardState, hook: impl FnOnce()) -> io::Result<GuardState> {
        let day = self.day.clone().unwrap_or_else(|| state.day.clone());
        self.save_transaction_hook(state, || Ok((day, 1000)), hook)
    }
}

pub(crate) fn load_usage_transaction(
    store: &mut StateStore,
    calendar: impl FnOnce() -> io::Result<Calendar>,
) -> io::Result<Option<SavedState>> {
    store.invalidate();
    transaction(calendar, |day, now| {
        let dir = store.directory()?;
        let config = read::<SavedState>(dir.join("guard-config.ron"))?;
        let guard = read::<GuardState>(dir.join("guard-state.ron"))?;
        store.load_and_migrate_with(|state: &mut SavedState| {
            state.retain_current_day(day);
            state.cooldown_until_ms = state.cooldown_until_ms.filter(|until| *until > now);
            if let Some(config) = config {
                state.merge_persisted(&config, day, now);
            }
            if let Some(guard) = guard {
                state.merge_guard(&guard, now);
            }
        })
    })
}

pub(crate) fn save_usage_transaction(
    store: &StateStore,
    dir: &Path,
    state: &SavedState,
    calendar: impl FnOnce() -> io::Result<Calendar>,
) -> io::Result<SavedState> {
    transaction(calendar, |day, now| {
        if !store.is_ready() {
            return Err(io::Error::other("UI state has not been read"));
        }
        // Validate all canonical files before the first write. The following writes
        // are retryable, not a claim of atomic rollback across multiple files.
        let saved_usage = read::<SavedState>(dir.join("usage-state.ron"))?;
        let saved_config = read::<SavedState>(dir.join("guard-config.ron"))?;
        let saved_guard = read::<GuardState>(dir.join("guard-state.ron"))?;
        let mut current = state.clone();
        current.retain_current_day(day);
        current.cooldown_until_ms = current.cooldown_until_ms.filter(|until| *until > now);
        for saved in [saved_usage, saved_config].iter().flatten() {
            current.merge_persisted(saved, day, now);
        }
        let mut guard = saved_guard;
        if let Some(guard) = &mut guard {
            normalize_guard(guard, &current, day, now);
            current.merge_guard(guard, now);
        }
        store.save(&current)?;
        put(dir.join("guard-config.ron"), &current)?;
        if let Some(guard) = guard {
            put(dir.join("guard-state.ron"), &guard)?;
        }
        remove_legacy_status(dir)?;
        Ok(current)
    })
}

pub(crate) fn sync_calendar_transaction(
    dir: &Path,
    calendar: impl FnOnce() -> io::Result<Calendar>,
) -> io::Result<Calendar> {
    transaction(calendar, |day, now| {
        let usage = read::<SavedState>(dir.join("usage-state.ron"))?;
        let config = read::<SavedState>(dir.join("guard-config.ron"))?;
        let guard = read::<GuardState>(dir.join("guard-state.ron"))?;
        let mut merged = config.clone().or_else(|| usage.clone());
        if let Some(merged) = &mut merged {
            for saved in [usage.as_ref(), config.as_ref()].into_iter().flatten() {
                merged.merge_persisted(saved, day, now);
            }
            if let Some(guard) = &guard {
                merged.merge_guard(guard, now);
            }
        }
        for (name, value) in [
            ("usage-state.ron", usage),
            ("guard-config.ron", config.clone()),
        ] {
            if let Some(mut value) = value {
                value.retain_current_day(day);
                if let Some(merged) = &merged {
                    value.merge_persisted(merged, day, now);
                }
                value.cooldown_until_ms = value.cooldown_until_ms.filter(|until| *until > now);
                put(dir.join(name), &value)?;
            }
        }
        if let (Some(mut state), Some(config)) = (guard, merged) {
            normalize_guard(&mut state, &config, day, now);
            put(dir.join("guard-state.ron"), &state)?;
        }
        remove_legacy_status(dir)?;
        Ok((day.to_owned(), now))
    })
}

fn remove_legacy_status(dir: &Path) -> io::Result<()> {
    for name in ["guard-status.ron", "guard-status.pending"] {
        match std::fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
fn test_calendar(day: Option<&str>) -> io::Result<Calendar> {
    day.filter(|d| !d.is_empty())
        .map(|d| (d.to_owned(), 1000))
        .ok_or_else(|| io::Error::other("Calendar unavailable"))
}
#[cfg(test)]
fn load_usage(store: &mut StateStore, day: Option<&str>) -> io::Result<Option<SavedState>> {
    load_usage_transaction(store, || test_calendar(day))
}
#[cfg(test)]
fn save_usage(
    store: &StateStore,
    dir: &Path,
    state: &SavedState,
    day: Option<&str>,
) -> io::Result<()> {
    save_usage_transaction(store, dir, state, || test_calendar(day)).map(|_| ())
}
#[cfg(test)]
fn sync_calendar(dir: &Path, day: Option<&str>) -> io::Result<()> {
    sync_calendar_transaction(dir, || test_calendar(day)).map(|_| ())
}
#[cfg(test)]
fn save_policy(dir: &Path, config: &SavedState, day: Option<&str>) -> io::Result<()> {
    let mut store = StateStore::new(dir.join("usage-state.ron"));
    load_usage(&mut store, day)?;
    save_usage(&store, dir, config, day)
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
        let original = std::fs::read(fixture.0.join("guard-state.ron")).unwrap();
        assert!(save_policy(&fixture.0, &config(&["video"]), None).is_err());
        assert_eq!(
            std::fs::read(fixture.0.join("guard-state.ron")).unwrap(),
            original
        );
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

    #[test]
    fn delayed_ui_reads_clock_only_after_the_new_day_worker_commit() {
        let fixture = Fixture::new();
        seed(&fixture);
        let stale = dated_config();
        let path = fixture.0.join("usage-state.ron");
        crate::persistence::write_atomic(&path, &stale).unwrap();
        save_policy(&fixture.0, &stale, Some("2026-11-11")).unwrap();
        let mut ui = StateStore::new(path);
        load_usage(&mut ui, Some("2026-11-11")).unwrap();
        let clock = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(11));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (sampled_tx, sampled_rx) = std::sync::mpsc::channel();
        let lock = FILES.lock().unwrap();
        let dir = fixture.0.clone();
        let read_clock = clock.clone();
        let selected = stale.policy();
        let writer = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            save_usage_transaction(&ui, &dir, &stale, || {
                let day = read_clock.load(Ordering::SeqCst);
                sampled_tx.send(()).unwrap();
                Ok((format!("2026-11-{day:02}"), 1000))
            })
            .unwrap();
        });
        started_rx.recv().unwrap();
        // The actual storage lock is held by a new-day commit. A clock read by the UI
        // before releasing it reproduces the old read-before-lock interleaving.
        let sampled_early = sampled_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_ok();
        let mut current = old_state();
        current.prepare_persistence(&selected, Some("2026-11-12"));
        current.playback_ms.insert("video".into(), 99);
        crate::persistence::write_atomic(&fixture.0.join("guard-state.ron"), &current).unwrap();
        clock.store(12, Ordering::SeqCst);
        drop(lock);
        writer.join().unwrap();
        assert!(
            !sampled_early,
            "calendar observed before the storage transaction lock"
        );
        assert_eq!(fixture.state().day, "2026-11-12");
        assert_eq!(fixture.state().playback_ms["video"], 99);
    }

    #[test]
    fn unknown_calendar_does_not_restore_a_stale_ui_over_a_new_day_checkpoint() {
        let fixture = Fixture::new();
        seed(&fixture);
        let stale = dated_config();
        let path = fixture.0.join("usage-state.ron");
        crate::persistence::write_atomic(&path, &stale).unwrap();
        save_policy(&fixture.0, &stale, Some("2026-11-11")).unwrap();
        let mut ui = StateStore::new(path.clone());
        load_usage(&mut ui, Some("2026-11-11")).unwrap();
        sync_calendar(&fixture.0, Some("2026-11-12")).unwrap();
        let before = std::fs::read(&path).unwrap();
        let result = save_usage(&ui, &fixture.0, &stale, None);
        assert!(
            result.is_err(),
            "unknown calendar was accepted as a successful save"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn failed_clock_keeps_every_file_then_retry_merges_current_values() {
        let fixture = Fixture::new();
        seed(&fixture);
        let stale = dated_config();
        let path = fixture.0.join("usage-state.ron");
        crate::persistence::write_atomic(&path, &stale).unwrap();
        save_policy(&fixture.0, &stale, Some("2026-11-11")).unwrap();
        let mut ui = StateStore::new(path);
        load_usage(&mut ui, Some("2026-11-11")).unwrap();
        sync_calendar(&fixture.0, Some("2026-11-12")).unwrap();
        let names = ["usage-state.ron", "guard-config.ron", "guard-state.ron"];
        let before: Vec<_> = names
            .iter()
            .map(|name| std::fs::read(fixture.0.join(name)).unwrap())
            .collect();
        assert!(
            save_usage_transaction(&ui, &fixture.0, &stale, || Err(io::Error::other(
                "injected clock error"
            )))
            .is_err()
        );
        for (name, bytes) in names.iter().zip(&before) {
            assert_eq!(std::fs::read(fixture.0.join(name)).unwrap(), *bytes);
        }
        let saved =
            save_usage_transaction(&ui, &fixture.0, &stale, || Ok(("2026-11-12".into(), 1000)))
                .unwrap();
        assert_eq!(saved.current_day_key, "2026-11-12");
        assert!(
            std::fs::read_to_string(fixture.0.join("usage-state.ron"))
                .unwrap()
                .contains("daily_records:[]")
        );
    }

    #[test]
    fn equivalent_map_order_is_not_a_new_checkpoint_write() {
        let fixture = Fixture::new();
        let path = fixture.0.join("unordered.ron");
        let map =
            std::collections::HashMap::<String, u64>::from([("a".into(), 1), ("b".into(), 2)]);
        let canonical = ron::to_string(&map).unwrap();
        let old = if canonical.starts_with("{\"a\"") {
            "{\"b\":2,\"a\":1}"
        } else {
            "{\"a\":1,\"b\":2}"
        };
        assert_ne!(old, canonical);
        std::fs::write(&path, old).unwrap();
        put(path.clone(), &map).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), old);
    }

    #[test]
    fn stopped_ui_save_migrates_legacy_checkpoint_history_even_when_metrics_match() {
        let fixture = Fixture::new();
        seed(&fixture);
        let config = config(&["video"]);
        save_policy(&fixture.0, &config, Some("2026-11-11")).unwrap();
        let path = fixture.0.join("guard-state.ron");
        let legacy = std::fs::read_to_string(&path).unwrap();
        let legacy = format!(
            "{},history:[(day:\"2026-11-10\",usage_ms:{{\"video\":999}},playback_ms:{{}},raw_ms:{{}},boot:None,triggered_day:None)])",
            legacy.strip_suffix(')').unwrap()
        );
        std::fs::write(&path, &legacy).unwrap();
        let mut ui = StateStore::new(fixture.0.join("usage-state.ron"));
        load_usage(&mut ui, Some("2026-11-11")).unwrap();
        save_usage_transaction(&ui, &fixture.0, &config, || Ok(("2026-11-11".into(), 1000)))
            .unwrap();
        let disk = std::fs::read_to_string(&path).unwrap();
        assert!(!disk.contains("history"));
        assert_eq!(fixture.state().usage_ms["video"], 61_000);
        assert_eq!(fixture.state().cooldown_until_ms, Some(123_456));
    }

    #[test]
    fn failed_legacy_status_cleanup_keeps_load_unready_until_a_successful_retry() {
        let fixture = Fixture::new();
        seed(&fixture);
        save_policy(&fixture.0, &config(&["video"]), Some("2026-11-11")).unwrap();
        std::fs::create_dir(fixture.0.join("guard-status.ron")).unwrap();
        let mut worker = GuardStore::new(fixture.0.clone());
        assert!(
            worker
                .load_transaction(|| Ok(("2026-11-11".into(), 1000)))
                .is_err()
        );
        assert!(
            !worker.is_ready(),
            "cleanup error left the state marked ready before it was handed to the worker"
        );
        assert!(
            worker
                .save_transaction(&GuardState::default(), || Ok(("2026-11-11".into(), 1000)))
                .is_err()
        );
        std::fs::remove_dir(fixture.0.join("guard-status.ron")).unwrap();
        let restored = worker
            .load_transaction(|| Ok(("2026-11-11".into(), 1000)))
            .unwrap()
            .unwrap();
        assert!(worker.is_ready());
        assert_eq!(restored.usage_ms["video"], 61_000);
    }

    #[test]
    fn checkpoint_commit_survives_cleanup_error_reload_and_stale_retry() {
        let fixture = Fixture::new();
        seed(&fixture);
        save_policy(&fixture.0, &config(&["video"]), Some("2026-11-11")).unwrap();
        let mut worker = GuardStore::new(fixture.0.clone());
        let old = worker
            .load_transaction(|| Ok(("2026-11-11".into(), 1000)))
            .unwrap()
            .unwrap();
        let mut new = old.clone();
        new.playback_ms.insert("video".into(), 99);
        new.cooldown_until_ms = None;
        std::fs::create_dir(fixture.0.join("guard-status.ron")).unwrap();
        assert!(
            worker
                .save_transaction(&new, || Ok(("2026-11-11".into(), 200_000)))
                .is_err()
        );
        // The checkpoint rename completed; only obsolete status cleanup failed afterwards.
        assert_eq!(fixture.state().playback_ms["video"], 99);
        assert_eq!(fixture.state().cooldown_until_ms, None);
        let committed = std::fs::read(fixture.0.join("guard-state.ron")).unwrap();
        let mut reloaded = GuardStore::new(fixture.0.clone());
        assert!(
            reloaded
                .load_transaction(|| Ok(("2026-11-11".into(), 200_000)))
                .is_err()
        );
        assert!(!reloaded.is_ready());
        assert_eq!(
            std::fs::read(fixture.0.join("guard-state.ron")).unwrap(),
            committed
        );
        std::fs::remove_dir(fixture.0.join("guard-status.ron")).unwrap();
        let restored = reloaded
            .load_transaction(|| Ok(("2026-11-11".into(), 200_000)))
            .unwrap()
            .unwrap();
        assert_eq!(restored.playback_ms["video"], 99);
        worker
            .save_transaction(&old, || Ok(("2026-11-11".into(), 200_000)))
            .unwrap();
        assert_eq!(fixture.state().playback_ms["video"], 99);
        assert_eq!(fixture.state().cooldown_until_ms, None);
    }

    #[test]
    fn expired_break_partial_save_cannot_revive_on_rollback_and_restart() {
        let fixture = Fixture::new();
        seed(&fixture);
        let mut initial = dated_config();
        initial.limit_triggered_day = Some("2026-11-11".into());
        save_policy(&fixture.0, &initial, Some("2026-11-11")).unwrap();
        let mut ui = StateStore::new(fixture.0.join("usage-state.ron"));
        let mut current = load_usage(&mut ui, Some("2026-11-11")).unwrap().unwrap();
        current.cooldown_until_ms = None;
        let pending = fixture.0.join("guard-config.pending");
        std::fs::create_dir(&pending).unwrap();
        assert!(
            save_usage_transaction(&ui, &fixture.0, &current, || Ok((
                "2026-11-11".into(),
                200_000
            )))
            .is_err()
        );
        let committed: SavedState = read(fixture.0.join("usage-state.ron")).unwrap().unwrap();
        assert_eq!(committed.cooldown_until_ms, None);
        assert_eq!(fixture.state().cooldown_until_ms, Some(123_456));
        std::fs::remove_dir(pending).unwrap();
        let restored = save_usage_transaction(&ui, &fixture.0, &current, || {
            Ok(("2026-11-11".into(), 1000))
        })
        .unwrap();
        assert_eq!(
            restored.cooldown_until_ms, None,
            "expired break returned from an old canonical copy after clock rollback"
        );
        let mut restarted = GuardStore::new(fixture.0.clone());
        let state = restarted
            .load_transaction(|| Ok(("2026-11-11".into(), 1000)))
            .unwrap()
            .unwrap();
        assert_eq!(state.cooldown_until_ms, None);
        assert_eq!(state.last_triggered_day.as_deref(), Some("2026-11-11"));
    }

    #[test]
    fn each_partial_write_and_cleanup_failure_preserves_completion_for_all_recovery_owners() {
        for fault in [
            "guard-config.pending",
            "guard-state.pending",
            "guard-status.ron",
        ] {
            for recover in 0..3 {
                let fixture = Fixture::new();
                seed(&fixture);
                let mut initial = dated_config();
                initial.limit_triggered_day = Some("2026-11-11".into());
                save_policy(&fixture.0, &initial, Some("2026-11-11")).unwrap();
                let mut ui = StateStore::new(fixture.0.join("usage-state.ron"));
                let mut current = load_usage(&mut ui, Some("2026-11-11")).unwrap().unwrap();
                let mut worker = GuardStore::new(fixture.0.clone());
                let stale = worker.load().unwrap().unwrap();
                current.cooldown_until_ms = None;
                std::fs::create_dir(fixture.0.join(fault)).unwrap();
                assert!(
                    save_usage_transaction(&ui, &fixture.0, &current, || Ok((
                        "2026-11-11".into(),
                        200_000
                    )))
                    .is_err(),
                    "{fault}"
                );
                let committed = std::fs::read(fixture.0.join("usage-state.ron")).unwrap();
                std::fs::remove_dir(fixture.0.join(fault)).unwrap();
                match recover {
                    0 => {
                        sync_calendar_transaction(&fixture.0, || Ok(("2026-11-11".into(), 1000)))
                            .unwrap();
                    }
                    1 => {
                        worker
                            .save_transaction(&stale, || Ok(("2026-11-11".into(), 1000)))
                            .unwrap();
                    }
                    _ => {
                        let mut restarted = GuardStore::new(fixture.0.clone());
                        let mut state = restarted.load().unwrap().unwrap();
                        state.merge_unpersisted(stale, 1000);
                        assert_eq!(state.cooldown_until_ms, None);
                        restarted
                            .save_transaction(&state, || Ok(("2026-11-11".into(), 1000)))
                            .unwrap();
                    }
                }
                let restored = load_usage(&mut ui, Some("2026-11-11")).unwrap().unwrap();
                assert_eq!(
                    restored.cooldown_until_ms, None,
                    "{fault}, recovery {recover}"
                );
                assert_eq!(
                    std::fs::read(fixture.0.join("usage-state.ron")).unwrap(),
                    committed
                );
                assert_eq!(fixture.state().usage_ms["video"], 61_000);
                assert_eq!(fixture.state().cooldown_until_ms, None);
                assert_eq!(
                    fixture.state().last_triggered_day.as_deref(),
                    Some("2026-11-11")
                );
            }
        }
    }

    #[test]
    fn worker_completion_before_cleanup_failure_wins_over_stale_ui_copies_on_restart() {
        let fixture = Fixture::new();
        seed(&fixture);
        let mut initial = dated_config();
        initial.limit_triggered_day = Some("2026-11-11".into());
        save_policy(&fixture.0, &initial, Some("2026-11-11")).unwrap();
        let mut worker = GuardStore::new(fixture.0.clone());
        let mut state = worker.load().unwrap().unwrap();
        state.playback_ms.insert("video".into(), 99);
        state.cooldown_until_ms = None;
        std::fs::create_dir(fixture.0.join("guard-status.ron")).unwrap();
        assert!(
            worker
                .save_transaction(&state, || Ok(("2026-11-11".into(), 200_000)))
                .is_err()
        );
        assert_eq!(fixture.state().cooldown_until_ms, None);
        std::fs::remove_dir(fixture.0.join("guard-status.ron")).unwrap();
        let mut ui = StateStore::new(fixture.0.join("usage-state.ron"));
        let restored = load_usage(&mut ui, Some("2026-11-11")).unwrap().unwrap();
        assert_eq!(restored.cooldown_until_ms, None);
        let saved = save_usage_transaction(&ui, &fixture.0, &initial, || {
            Ok(("2026-11-11".into(), 1000))
        })
        .unwrap();
        assert_eq!(saved.cooldown_until_ms, None);
        assert_eq!(fixture.state().playback_ms["video"], 99);
    }

    #[test]
    fn failed_ui_reload_of_related_state_invalidates_readiness_without_writing() {
        let fixture = Fixture::new();
        save_policy(&fixture.0, &config(&["video"]), Some("2026-11-11")).unwrap();
        let mut store = StateStore::new(fixture.0.join("usage-state.ron"));
        load_usage(&mut store, Some("2026-11-11")).unwrap();
        let before = std::fs::read(fixture.0.join("usage-state.ron")).unwrap();
        std::fs::write(fixture.0.join("guard-config.ron"), "(broken").unwrap();
        assert!(load_usage(&mut store, Some("2026-11-11")).is_err());
        assert!(!store.is_ready());
        assert!(save_usage(&store, &fixture.0, &config(&[]), Some("2026-11-11")).is_err());
        assert_eq!(
            std::fs::read(fixture.0.join("usage-state.ron")).unwrap(),
            before
        );
        crate::persistence::write_atomic(&fixture.0.join("guard-config.ron"), &config(&["video"]))
            .unwrap();
        let restored = load_usage(&mut store, Some("2026-11-11")).unwrap().unwrap();
        assert!(store.is_ready());
        assert_eq!(restored.target_apps[0].package, "video");
    }

    #[test]
    fn late_ui_and_worker_saves_keep_new_day_playback_and_do_not_revive_expired_breaks() {
        let fixture = Fixture::new();
        seed(&fixture);
        let stale = dated_config();
        let path = fixture.0.join("usage-state.ron");
        crate::persistence::write_atomic(&path, &stale).unwrap();
        save_policy(&fixture.0, &stale, Some("2026-11-11")).unwrap();
        let mut ui = StateStore::new(path);
        load_usage(&mut ui, Some("2026-11-11")).unwrap();
        let mut worker = GuardStore::new(fixture.0.clone());
        let mut current = worker
            .load_transaction(|| Ok(("2026-11-12".into(), 200_000)))
            .unwrap()
            .unwrap();
        current.playback_ms.insert("video".into(), 99);
        current.cooldown_until_ms = None;
        current.last_triggered_day = Some("2026-11-12".into());
        worker
            .save_transaction(&current, || Ok(("2026-11-12".into(), 200_000)))
            .unwrap();
        let saved = save_usage_transaction(&ui, &fixture.0, &stale, || {
            Ok(("2026-11-12".into(), 200_000))
        })
        .unwrap();
        assert_eq!(saved.cooldown_until_ms, None);
        assert_eq!(
            saved.last_limit_triggered_day.as_deref(),
            Some("2026-11-12")
        );
        assert_eq!(fixture.state().playback_ms["video"], 99);
        let mut late = GuardState::default();
        late.day = "2026-11-11".into();
        late.cooldown_until_ms = Some(123_456);
        worker
            .save_transaction(&late, || Ok(("2026-11-12".into(), 200_000)))
            .unwrap();
        assert_eq!(fixture.state().playback_ms["video"], 99);
        assert_eq!(fixture.state().cooldown_until_ms, None);
        assert_eq!(fixture.state().day, "2026-11-12");
        save_usage_transaction(&ui, &fixture.0, &saved, || {
            Ok(("2026-11-10".into(), 200_001))
        })
        .unwrap();
        assert_eq!(fixture.state().day, "2026-11-10");
        assert_eq!(
            fixture.state().last_triggered_day.as_deref(),
            Some("2026-11-12")
        );
    }
}
