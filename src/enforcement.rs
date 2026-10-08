use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
}

impl GuardState {
    pub fn import_break(&mut self, until: Option<i64>, day: &Option<String>, now: i64) {
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
            self.day = day.to_owned();
            self.usage_ms.clear();
            self.playback_ms.clear();
            self.raw_ms.clear();
            self.triggered_day = None;
            self.boot = None;
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
    fn deselected_apps_are_removed_from_persistent_metrics() {
        let mut state = GuardState::default();
        state.usage_ms.insert("mail".into(), 99);
        state.playback_ms.insert("mail".into(), 99);
        state.raw_ms.insert("mail".into(), 99);
        state.retain_selected(&policy());
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
