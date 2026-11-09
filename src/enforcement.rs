use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// No session, missing state, NONE and ERROR are unknown, not measured stops.
#[derive(Default)]
pub(crate) struct MediaObservation {
    known: std::collections::HashSet<String>,
    pub playing: Option<String>,
}

impl MediaObservation {
    pub fn observe(&mut self, package: &str, state: i32, foreground: Option<&str>) {
        if !matches!(state, 1..=6 | 8..=11) {
            return;
        }
        self.known.insert(package.to_owned());
        if state == 3 && foreground == Some(package) {
            self.playing = Some(package.to_owned());
        }
    }

    pub fn available(&self, selected: &[String]) -> bool {
        !selected.is_empty() && selected.iter().all(|package| self.known.contains(package))
    }
}

#[cfg(test)]
mod media_tests {
    use super::MediaObservation;
    #[test]
    fn absent_invalid_or_unselected_sessions_do_not_prove_a_stop() {
        let selected = vec!["video".into()];
        let mut observation = MediaObservation::default();
        assert!(!observation.available(&selected));
        observation.observe("other", 2, None);
        observation.observe("video", 0, Some("video"));
        observation.observe("video", 7, Some("video"));
        assert!(!observation.available(&selected));
        observation.observe("video", 2, Some("video"));
        assert!(observation.available(&selected));
        assert!(observation.playing.is_none());
        assert!(!observation.available(&["video".into(), "unsupported".into()]));
    }

    #[test]
    fn only_visible_playing_sessions_count_and_paused_siblings_do_not_cancel_them() {
        let mut observation = MediaObservation::default();
        observation.observe("video", 3, None);
        assert!(observation.playing.is_none());
        observation.observe("video", 3, Some("video"));
        observation.observe("video", 2, Some("video"));
        assert_eq!(observation.playing.as_deref(), Some("video"));
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Policy {
    pub packages: Vec<String>,
    pub limit_minutes: u32,
    pub break_minutes: u32,
    pub playback_only: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct GuardState {
    pub day: String,
    pub usage_ms: HashMap<String, u64>,
    pub playback_ms: HashMap<String, u64>,
    raw_ms: HashMap<String, u64>,
    boot: Option<i32>,
    pub cooldown_until_ms: Option<i64>,
    pub triggered_day: Option<String>,
    #[serde(skip_serializing)]
    history: Vec<DayLedger>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct DayLedger {
    day: String,
    usage_ms: HashMap<String, u64>,
    playback_ms: HashMap<String, u64>,
    raw_ms: HashMap<String, u64>,
    boot: Option<i32>,
    triggered_day: Option<String>,
}

impl GuardState {
    /// This worker has not saved while reads failed. Its playback observations
    /// are newer than the recovered file; cumulative OS usage still overlaps.
    pub fn merge_unpersisted(&mut self, mut memory: Self) {
        let until = self.cooldown_until_ms.max(memory.cooldown_until_ms);
        memory.history.push(DayLedger {
            day: memory.day,
            usage_ms: memory.usage_ms,
            playback_ms: memory.playback_ms,
            raw_ms: memory.raw_ms,
            boot: memory.boot,
            triggered_day: memory.triggered_day,
        });
        for record in memory.history {
            if record.day.is_empty() {
                continue;
            }
            self.sample_usage(&record.day, record.boot.unwrap_or(-1), &record.raw_ms);
            for (package, total) in record.usage_ms {
                let stored = self.usage_ms.entry(package).or_default();
                *stored = (*stored).max(total);
            }
            for (package, total) in record.playback_ms {
                let stored = self.playback_ms.entry(package).or_default();
                *stored = stored.saturating_add(total);
            }
            if record.triggered_day.as_deref() == Some(record.day.as_str()) {
                self.triggered_day = record.triggered_day;
            }
        }
        self.cooldown_until_ms = until;
    }

    pub fn import_break(&mut self, until: Option<i64>, day: &Option<String>, now: i64) {
        if self.triggered_day.is_none() && day.as_deref() == Some(self.day.as_str()) {
            self.triggered_day = day.clone();
        }
        if self.cooldown_until_ms.is_none() && until.is_some_and(|until| until > now) {
            self.cooldown_until_ms = until;
            self.triggered_day = day.clone();
        }
    }

    pub fn observe_playback(
        &mut self,
        changed_day: bool,
        playing: Option<String>,
        previous: &mut Option<String>,
        elapsed: u64,
    ) {
        if !changed_day && playing.is_some() && playing == *previous && elapsed <= 3_000 {
            let total = self
                .playback_ms
                .entry(playing.clone().unwrap())
                .or_default();
            *total = total.saturating_add(elapsed);
        }
        *previous = playing;
    }
    pub fn sample_usage(&mut self, day: &str, boot: i32, totals: &HashMap<String, u64>) {
        if self.day != day {
            if !self.day.is_empty() {
                self.history.retain(|record| record.day != self.day);
                self.history.push(DayLedger {
                    day: self.day.clone(),
                    usage_ms: std::mem::take(&mut self.usage_ms),
                    playback_ms: std::mem::take(&mut self.playback_ms),
                    raw_ms: std::mem::take(&mut self.raw_ms),
                    boot: self.boot.take(),
                    triggered_day: self.triggered_day.take(),
                });
            }
            self.day = day.to_owned();
            self.usage_ms.clear();
            self.playback_ms.clear();
            self.raw_ms.clear();
            self.triggered_day = None;
            self.boot = None;
            if let Some(index) = self.history.iter().position(|record| record.day == day) {
                let restored = self.history.remove(index);
                self.usage_ms = restored.usage_ms;
                self.playback_ms = restored.playback_ms;
                self.raw_ms = restored.raw_ms;
                self.boot = restored.boot;
                self.triggered_day = restored.triggered_day;
            }
            self.history.sort_by(|a, b| a.day.cmp(&b.day));
            if self.history.len() > 13 {
                self.history.drain(..self.history.len() - 13);
            }
        }
        let rebooted = self.boot.is_some_and(|old| boot >= 0 && old != boot);
        if rebooted {
            self.raw_ms.clear();
        }
        for (package, &raw) in totals {
            let previous = self.raw_ms.get(package).copied().unwrap_or(0);
            let delta = if rebooted {
                raw
            } else {
                raw.saturating_sub(previous)
            };
            let observed = self.usage_ms.entry(package.clone()).or_default();
            *observed = observed.saturating_add(delta);
            self.raw_ms.insert(
                package.clone(),
                if rebooted { raw } else { previous.max(raw) },
            );
        }
        if boot >= 0 {
            self.boot = Some(boot);
        }
    }

    pub fn enforce(&mut self, policy: &Policy, now_ms: i64) {
        if self.cooldown_until_ms.is_some_and(|until| until <= now_ms) {
            self.cooldown_until_ms = None;
        }
        let totals = if policy.playback_only {
            &self.playback_ms
        } else {
            &self.usage_ms
        };
        let total = policy.packages.iter().fold(0_u64, |sum, p| {
            sum.saturating_add(*totals.get(p).unwrap_or(&0))
        });
        if policy.enabled
            && self.cooldown_until_ms.is_none()
            && self.triggered_day.as_deref() != Some(self.day.as_str())
            && total >= u64::from(policy.limit_minutes.max(1)) * 60_000
        {
            self.triggered_day = Some(self.day.clone());
            self.cooldown_until_ms =
                Some(now_ms.saturating_add(i64::from(policy.break_minutes.max(1)) * 60_000));
        }
    }

    pub fn retain_selected(&mut self, policy: &Policy) {
        for record in &mut self.history {
            for map in [
                &mut record.usage_ms,
                &mut record.playback_ms,
                &mut record.raw_ms,
            ] {
                map.retain(|p, _| policy.packages.contains(p));
            }
        }
        for map in [&mut self.usage_ms, &mut self.playback_ms, &mut self.raw_ms] {
            map.retain(|p, _| policy.packages.contains(p));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> Policy {
        Policy {
            packages: vec!["video".into()],
            limit_minutes: 1,
            break_minutes: 5,
            playback_only: false,
            enabled: true,
        }
    }
    fn sample(state: &mut GuardState, day: &str, boot: i32, ms: u64) {
        state.sample_usage(day, boot, &HashMap::from([("video".into(), ms)]));
    }
    #[test]
    fn a_completed_break_in_config_survives_missing_service_history() {
        let mut memory = GuardState::default();
        sample(&mut memory, "day1", 1, 65_000);
        memory.import_break(None, &Some("day1".into()), 100);
        memory.enforce(&policy(), 100);
        assert_eq!(memory.cooldown_until_ms, None);
        assert_eq!(memory.triggered_day.as_deref(), Some("day1"));
    }

    #[test]
    fn recovery_combines_disjoint_playback_but_not_overlapping_os_usage() {
        let mut recovered = GuardState::default();
        sample(&mut recovered, "day1", 1, 1_000);
        recovered.playback_ms.insert("video".into(), 2_000);
        recovered.cooldown_until_ms = Some(200_000);
        let mut memory = GuardState::default();
        sample(&mut memory, "day1", 1, 5_000);
        memory.playback_ms.insert("video".into(), 3_000);
        memory.cooldown_until_ms = Some(100_000);
        memory.triggered_day = Some("day1".into());
        recovered.merge_unpersisted(memory);
        assert_eq!(recovered.usage_ms["video"], 5_000);
        assert_eq!(recovered.playback_ms["video"], 5_000);
        assert_eq!(recovered.cooldown_until_ms, Some(200_000));
        assert_eq!(recovered.triggered_day.as_deref(), Some("day1"));
        sample(&mut recovered, "day1", 1, 6_000);
        assert_eq!(recovered.usage_ms["video"], 6_000);
    }

    #[test]
    fn changing_the_clock_back_restores_the_day_without_another_break() {
        let mut state = GuardState::default();
        sample(&mut state, "day2", 1, 60_000);
        state.enforce(&policy(), 0);
        state.enforce(&policy(), 300_000);
        sample(&mut state, "day3", 1, 1_000);
        sample(&mut state, "day2", 1, 65_000);
        state.enforce(&policy(), 400_000);
        assert_eq!(state.usage_ms["video"], 65_000);
        assert_eq!(state.triggered_day.as_deref(), Some("day2"));
        assert_eq!(state.cooldown_until_ms, None);
    }

    #[test]
    fn paused_unknown_and_midnight_playback_are_not_added() {
        let mut state = GuardState::default();
        let mut previous = None;
        state.observe_playback(false, Some("video".into()), &mut previous, 1000);
        state.observe_playback(false, Some("video".into()), &mut previous, 1000);
        assert_eq!(state.playback_ms["video"], 1000);
        state.observe_playback(false, None, &mut previous, 1000);
        state.observe_playback(false, Some("video".into()), &mut previous, 1000);
        state.observe_playback(true, Some("video".into()), &mut previous, 1000);
        state.observe_playback(false, Some("video".into()), &mut previous, 9000);
        assert_eq!(state.playback_ms["video"], 1000);
    }

    #[test]
    fn importing_launcher_break_never_replaces_an_existing_deadline() {
        let mut state = GuardState::default();
        state.import_break(Some(1000), &Some("day1".into()), 100);
        state.import_break(Some(5000), &Some("day1".into()), 100);
        assert_eq!(state.cooldown_until_ms, Some(1000));
        let mut empty = GuardState::default();
        empty.import_break(Some(1000), &None, 1000);
        assert_eq!(empty.cooldown_until_ms, None);
    }

    #[test]
    fn serialization_omits_previous_days_but_preserves_active_break() {
        let mut state = GuardState::default();
        sample(&mut state, "day1", 1, 60_000);
        state.enforce(&policy(), 1000);
        let deadline = state.cooldown_until_ms;
        sample(&mut state, "day2", 1, 123);
        assert!(!state.history.is_empty());
        let restored: GuardState = ron::from_str(&ron::to_string(&state).unwrap()).unwrap();
        assert!(restored.history.is_empty());
        assert_eq!(restored.day, "day2");
        assert_eq!(restored.usage_ms, state.usage_ms);
        assert_eq!(restored.cooldown_until_ms, deadline);
    }
    #[test]
    fn deselected_apps_are_removed_from_persistent_metrics() {
        let mut state = GuardState::default();
        state.usage_ms.insert("mail".into(), 99);
        state.playback_ms.insert("mail".into(), 99);
        state.raw_ms.insert("mail".into(), 99);
        let mut old = DayLedger::default();
        old.day = "yesterday".into();
        old.usage_ms.insert("mail".into(), 99);
        old.playback_ms.insert("mail".into(), 99);
        old.raw_ms.insert("mail".into(), 99);
        state.history.push(old);
        state.retain_selected(&policy());
        assert!(state.history.iter().all(|record| record.usage_ms.is_empty()
            && record.playback_ms.is_empty()
            && record.raw_ms.is_empty()));
        assert!(
            state.usage_ms.is_empty() && state.playback_ms.is_empty() && state.raw_ms.is_empty()
        );
    }

    #[test]
    fn reboot_counts_new_usage_without_erasing_the_old_interval() {
        let mut state = GuardState::default();
        sample(&mut state, "day1", 1, 50_000);
        sample(&mut state, "day1", 2, 0);
        sample(&mut state, "day1", 2, 11_000);
        state.enforce(&policy(), 100_000);
        assert_eq!(state.usage_ms["video"], 61_000);
        assert_eq!(state.cooldown_until_ms, Some(400_000));
    }
    #[test]
    fn duplicate_or_reduced_os_history_does_not_add_time() {
        let mut state = GuardState::default();
        for ms in [20_000, 20_000, 0, 22_000] {
            sample(&mut state, "day1", 1, ms);
        }
        assert_eq!(state.usage_ms["video"], 22_000);
    }

    #[test]
    fn reboot_without_any_selected_events_resets_only_the_raw_baseline() {
        let mut state = GuardState::default();
        sample(&mut state, "day1", 1, 50_000);
        state.sample_usage("day1", 2, &HashMap::new());
        sample(&mut state, "day1", 2, 11_000);
        assert_eq!(state.usage_ms["video"], 61_000);
    }
    #[test]
    fn playback_mode_cannot_trigger_from_foreground_usage() {
        let mut state = GuardState::default();
        sample(&mut state, "day1", 1, 90_000);
        let mut policy = policy();
        policy.playback_only = true;
        state.enforce(&policy, 100);
        assert_eq!(state.cooldown_until_ms, None);
        state.playback_ms.insert("video".into(), 60_000);
        state.enforce(&policy, 200);
        assert_eq!(state.cooldown_until_ms, Some(300_200));
    }
    #[test]
    fn midnight_preserves_active_break_and_completed_break_is_once_per_day() {
        let mut state = GuardState::default();
        sample(&mut state, "day1", 1, 60_000);
        state.enforce(&policy(), 100);
        sample(&mut state, "day2", 1, 80_000);
        state.enforce(&policy(), 200);
        assert_eq!(state.cooldown_until_ms, Some(300_100));
        state.enforce(&policy(), 300_100);
        assert_eq!(state.cooldown_until_ms, Some(600_100));
        state.enforce(&policy(), 600_100);
        assert_eq!(state.cooldown_until_ms, None);
    }
}
