//! Foreground intervals from Android UsageEvents. No screen contents are read.
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy)]
pub(crate) enum EventKind {
    Resumed,
    Paused,
    ScreenOff,
    Shutdown,
}

/// Streaming reducer; unrelated package names are never retained.
#[derive(Clone)]
pub(crate) struct ForegroundUsage {
    selected: HashSet<String>,
    start_ms: i64,
    end_ms: i64,
    last_event_ms: Option<i64>,
    active: Option<(String, HashSet<String>, i64)>,
    totals: HashMap<String, u64>,
}

impl ForegroundUsage {
    pub(crate) fn new(selected: &[String], start_ms: i64, end_ms: i64) -> Self {
        Self {
            selected: selected.iter().cloned().collect(),
            start_ms,
            end_ms: end_ms.max(start_ms),
            last_event_ms: None,
            active: None,
            totals: HashMap::new(),
        }
    }

    fn close(&mut self, at_ms: i64) {
        if let Some((package, _, began_ms)) = self.active.take() {
            let duration = at_ms
                .clamp(self.start_ms, self.end_ms)
                .saturating_sub(began_ms.max(self.start_ms))
                .max(0) as u64;
            let total = self.totals.entry(package).or_default();
            *total = total.saturating_add(duration);
        }
    }

    pub(crate) fn event(&mut self, at_ms: i64, package: &str, class: &str, kind: EventKind) {
        if at_ms > self.end_ms || self.last_event_ms.is_some_and(|last| at_ms < last) {
            return;
        }
        self.last_event_ms = Some(at_ms);
        match kind {
            EventKind::Resumed => {
                if self.active.as_ref().is_some_and(|(p, _, _)| p != package) {
                    self.close(at_ms);
                }
                if self.selected.contains(package) {
                    let (_, classes, _) = self
                        .active
                        .get_or_insert_with(|| (package.to_owned(), HashSet::new(), at_ms));
                    classes.insert(class.to_owned());
                }
            }
            EventKind::Paused => {
                if let Some((p, classes, _)) = &mut self.active
                    && p == package
                {
                    classes.remove(class);
                    if classes.is_empty() {
                        self.close(at_ms);
                    }
                }
            }
            EventKind::ScreenOff | EventKind::Shutdown => self.close(at_ms),
        }
    }

    pub(crate) fn finish(mut self) -> HashMap<String, u64> {
        self.close(self.end_ms);
        self.totals
    }

    pub(crate) fn foreground_package(&self) -> Option<String> {
        self.active.as_ref().map(|(package, _, _)| package.clone())
    }

    pub(crate) fn advance_end(&mut self, end_ms: i64) {
        self.end_ms = end_ms.max(self.start_ms);
    }
    pub(crate) fn snapshot(&self) -> HashMap<String, u64> {
        self.clone().finish()
    }
}

#[derive(Clone, Default)]
pub(crate) struct UsageTracker {
    day: String,
    boot: i32,
    selected: Vec<String>,
    cursor: i64,
    last_now: i64,
    cutoff: i64,
    reducer: Option<ForegroundUsage>,
    preview: Option<ForegroundUsage>,
}

impl UsageTracker {
    pub(crate) fn prepare(
        &mut self,
        day: &str,
        boot: i32,
        selected: &[String],
        start_ms: i64,
        now_ms: i64,
        earliest_ms: i64,
    ) -> i64 {
        if self.reducer.is_none()
            || self.day != day
            || self.boot != boot
            || self.selected != selected
            || now_ms < self.last_now
        {
            self.day = day.to_owned();
            self.boot = boot;
            self.selected = selected.to_vec();
            self.cursor = earliest_ms;
            self.reducer = Some(ForegroundUsage::new(selected, start_ms, now_ms));
        }
        // Android can publish an event after its timestamp. Keep a replayable
        // suffix; only the older prefix contributes to durable usage totals.
        self.cutoff = now_ms.saturating_sub(10_000).max(self.cursor).min(now_ms);
        self.reducer.as_mut().unwrap().advance_end(self.cutoff);
        self.preview = self.reducer.clone();
        self.preview.as_mut().unwrap().advance_end(now_ms);
        self.cursor
    }
    pub(crate) fn event(&mut self, at_ms: i64, package: &str, class: &str, kind: EventKind) {
        if at_ms >= self.cursor {
            if at_ms < self.cutoff {
                self.reducer
                    .as_mut()
                    .unwrap()
                    .event(at_ms, package, class, kind);
            }
            self.preview
                .as_mut()
                .unwrap()
                .event(at_ms, package, class, kind);
        }
    }
    pub(crate) fn result(&mut self, end_ms: i64) -> (HashMap<String, u64>, Option<String>) {
        self.cursor = self.cutoff;
        self.last_now = end_ms;
        let reducer = self.reducer.as_ref().unwrap();
        (
            reducer.snapshot(),
            self.preview.as_ref().unwrap().foreground_package(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packages() -> Vec<String> {
        vec!["video".into(), "social".into()]
    }

    #[test]
    fn incremental_queries_count_open_intervals_once_and_reset_on_boot() {
        let selected = packages();
        let mut cache = UsageTracker::default();
        assert_eq!(cache.prepare("day1", 1, &selected, 0, 20_000, 0), 0);
        cache.event(1000, "video", "Player", EventKind::Resumed);
        assert_eq!(cache.result(20_000).0["video"], 9000);
        assert_eq!(cache.prepare("day1", 1, &selected, 0, 30_000, 0), 10_000);
        cache.event(16_000, "video", "Player", EventKind::Paused);
        assert_eq!(cache.result(30_000).0["video"], 15_000);
        assert_eq!(
            cache.prepare("day1", 2, &selected, 30_000, 50_000, 30_000),
            30_000
        );
        cache.event(35_000, "video", "Player", EventKind::Resumed);
        assert_eq!(cache.result(50_000).0["video"], 5000);
    }

    #[test]
    fn delayed_events_with_old_timestamps_are_replayed_before_commit() {
        let selected = packages();
        let mut cache = UsageTracker::default();
        cache.prepare("day1", 1, &selected, 0, 20_000, 0);
        assert!(cache.result(20_000).0.is_empty());
        assert_eq!(cache.prepare("day1", 1, &selected, 0, 22_000, 0), 10_000);
        cache.event(15_000, "video", "Player", EventKind::Resumed);
        assert_eq!(cache.result(22_000).1.as_deref(), Some("video"));
        cache.prepare("day1", 1, &selected, 0, 30_000, 0);
        cache.event(15_000, "video", "Player", EventKind::Resumed);
        assert_eq!(cache.result(30_000).0["video"], 5000);
    }

    #[test]
    fn clips_midnight_and_the_query_end() {
        let selected = packages();
        let mut usage = ForegroundUsage::new(&selected, 100, 200);
        usage.event(50, "video", "Player", EventKind::Resumed);
        assert_eq!(usage.foreground_package().as_deref(), Some("video"));
        assert_eq!(usage.finish()["video"], 100);
    }

    #[test]
    fn switching_to_unselected_app_and_screen_off_stop_counting() {
        let selected = packages();
        let mut usage = ForegroundUsage::new(&selected, 0, 200);
        usage.event(10, "video", "Player", EventKind::Resumed);
        usage.event(50, "mail", "Inbox", EventKind::Resumed);
        usage.event(80, "social", "Feed", EventKind::Resumed);
        usage.event(100, "", "", EventKind::ScreenOff);
        assert_eq!(usage.foreground_package(), None);
        let totals = usage.finish();
        assert_eq!(totals["video"], 40);
        assert_eq!(totals["social"], 20);
        assert!(!totals.contains_key("mail"));
    }

    #[test]
    fn activity_transition_counts_union_and_ignores_old_pause() {
        let selected = packages();
        let mut usage = ForegroundUsage::new(&selected, 0, 100);
        usage.event(10, "video", "List", EventKind::Resumed);
        usage.event(20, "video", "Player", EventKind::Resumed);
        usage.event(30, "video", "List", EventKind::Paused);
        usage.event(50, "social", "Feed", EventKind::Resumed);
        usage.event(60, "video", "Player", EventKind::Paused);
        let totals = usage.finish();
        assert_eq!(totals["video"], 40);
        assert_eq!(totals["social"], 50);
    }

    #[test]
    fn duplicate_resume_and_shutdown_do_not_extend_intervals() {
        let selected = packages();
        let mut usage = ForegroundUsage::new(&selected, 0, 200);
        usage.event(10, "video", "Player", EventKind::Resumed);
        usage.event(20, "video", "Player", EventKind::Resumed);
        usage.event(50, "", "", EventKind::Shutdown);
        usage.event(100, "video", "Player", EventKind::Resumed);
        usage.event(120, "video", "Player", EventKind::Paused);
        assert_eq!(usage.finish()["video"], 60);
    }

    #[test]
    fn rejects_out_of_order_and_future_events() {
        let selected = packages();
        let mut usage = ForegroundUsage::new(&selected, 0, 100);
        usage.event(30, "video", "Player", EventKind::Resumed);
        usage.event(20, "video", "Player", EventKind::Paused);
        usage.event(101, "video", "Player", EventKind::Paused);
        assert_eq!(usage.finish()["video"], 70);
    }
}
