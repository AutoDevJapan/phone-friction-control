use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "android")]
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use eframe::egui::{self, Align, Color32, Layout, RichText, Stroke};
use serde::{Deserialize, Serialize};

const SAVE_KEY: &str = "phone-friction-control-v1";
const ORANGE: Color32 = Color32::from_rgb(239, 96, 34);
const PAPER: Color32 = Color32::from_rgb(247, 244, 238);
const INK: Color32 = Color32::from_rgb(37, 36, 34);
const MUTED: Color32 = Color32::from_rgb(104, 101, 96);
const DARK: Color32 = Color32::from_rgb(31, 31, 32);
const DEFAULT_LIMIT_MINUTES: u32 = 120;
const DEFAULT_BREAK_MINUTES: u32 = 30;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedApp {
    pub label: String,
    pub package: String,
}

#[cfg(any(target_os = "android", test))]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub permission_granted: bool,
    pub day_key: String,
    pub day_label: String,
    pub now_ms: i64,
    pub usage_by_package_ms: HashMap<String, u64>,
    pub foreground_package: Option<String>,
    pub boot_count: i32,
    pub measurement_available: bool,
}

#[cfg(target_os = "android")]
pub enum PlatformEvent {
    InsetsLoaded(Result<Option<[i32; 4]>, String>),
    AppsLoaded(Result<Vec<TrackedApp>, String>),
    UsageLoaded {
        selected_packages: Vec<String>,
        result: Result<UsageSnapshot, String>,
    },
    ActionDone {
        label: String,
        result: Result<(), String>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct DailyRecord {
    day_key: String,
    label: String,
    used_ms: u64,
    #[serde(default)]
    playback_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct SavedState {
    pub(crate) target_apps: Vec<TrackedApp>,
    pub(crate) daily_limit_minutes: u32,
    pub(crate) break_minutes: u32,
    pub(crate) cooldown_until_ms: Option<i64>,
    pub(crate) limit_triggered_day: Option<String>,
    pub(crate) last_limit_triggered_day: Option<String>,
    pub(crate) current_day_key: String,
    daily_records: Vec<DailyRecord>,
    pub(crate) usage_disclosure_accepted: bool,
    pub(crate) playback_only: bool,
    pub(crate) service_disclosure_accepted: bool,
}

impl Serialize for SavedState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut out = serializer.serialize_struct("SavedState", 11)?;
        out.serialize_field("target_apps", &self.target_apps)?;
        out.serialize_field("daily_limit_minutes", &self.daily_limit_minutes)?;
        out.serialize_field("break_minutes", &self.break_minutes)?;
        out.serialize_field("cooldown_until_ms", &self.cooldown_until_ms)?;
        out.serialize_field("limit_triggered_day", &self.limit_triggered_day)?;
        out.serialize_field("last_limit_triggered_day", &self.last_limit_triggered_day)?;
        out.serialize_field("current_day_key", &self.current_day_key)?;
        let records: Vec<_> = self
            .daily_records
            .iter()
            .filter(|r| r.day_key == self.current_day_key)
            .collect();
        out.serialize_field("daily_records", &records)?;
        out.serialize_field("usage_disclosure_accepted", &self.usage_disclosure_accepted)?;
        out.serialize_field("playback_only", &self.playback_only)?;
        out.serialize_field(
            "service_disclosure_accepted",
            &self.service_disclosure_accepted,
        )?;
        out.end()
    }
}
impl Default for SavedState {
    fn default() -> Self {
        Self {
            target_apps: Vec::new(),
            daily_limit_minutes: DEFAULT_LIMIT_MINUTES,
            break_minutes: DEFAULT_BREAK_MINUTES,
            cooldown_until_ms: None,
            limit_triggered_day: None,
            last_limit_triggered_day: None,
            current_day_key: String::new(),
            daily_records: Vec::new(),
            usage_disclosure_accepted: false,
            playback_only: false,
            service_disclosure_accepted: false,
        }
    }
}

#[cfg(any(target_os = "android", test))]
impl SavedState {
    pub(crate) fn merge_persisted(&mut self, other: &Self, day: &str, now: i64) {
        self.retain_current_day(day);
        if self.target_apps == other.target_apps && self.playback_only == other.playback_only {
            for record in other.daily_records.iter().filter(|r| r.day_key == day) {
                if let Some(current) = self
                    .daily_records
                    .iter_mut()
                    .find(|r| r.day_key == day && r.playback_only == record.playback_only)
                {
                    current.used_ms = current.used_ms.max(record.used_ms);
                } else {
                    self.daily_records.push(record.clone());
                }
            }
        }
        self.accept_service_break(
            other.cooldown_until_ms,
            other.limit_triggered_day.clone(),
            other.last_limit_triggered_day.clone(),
            now,
        );
    }

    pub(crate) fn merge_guard(&mut self, guard: &crate::enforcement::GuardState, now: i64) {
        self.accept_service_break(
            guard.cooldown_until_ms,
            guard.triggered_day.clone(),
            guard.last_triggered_day.clone(),
            now,
        );
    }
}

#[cfg(not(target_os = "android"))]
impl SavedState {
    fn remove_preview_session(&mut self) {
        if self.current_day_key != "preview" {
            return;
        }
        self.target_apps
            .retain(|app| !app.package.starts_with("preview."));
        self.current_day_key.clear();
        self.daily_records.clear();
        self.cooldown_until_ms = None;
        self.limit_triggered_day = None;
        self.last_limit_triggered_day = None;
        self.usage_disclosure_accepted = false;
        self.service_disclosure_accepted = false;
    }
}

#[cfg(any(target_os = "android", test))]
impl SavedState {
    pub(crate) fn retain_current_day(&mut self, day: &str) {
        if day.is_empty() {
            return;
        }
        self.current_day_key = day.to_owned();
        self.daily_records.retain(|record| record.day_key == day);
    }

    fn accept_service_break(
        &mut self,
        deadline: Option<i64>,
        triggered_day: Option<String>,
        last_triggered_day: Option<String>,
        now: i64,
    ) {
        let mut current = crate::enforcement::BreakState {
            deadline: self.cooldown_until_ms,
            triggered_day: self.limit_triggered_day.clone(),
            last_triggered_day: self.last_limit_triggered_day.clone(),
        };
        current.merge(
            crate::enforcement::BreakState {
                deadline,
                triggered_day,
                last_triggered_day,
            },
            now,
        );
        self.cooldown_until_ms = current.deadline;
        self.limit_triggered_day = current.triggered_day;
        self.last_limit_triggered_day = current.last_triggered_day;
    }
    pub(crate) fn policy(&self) -> crate::enforcement::Policy {
        crate::enforcement::Policy {
            packages: self
                .target_apps
                .iter()
                .map(|app| app.package.clone())
                .collect(),
            limit_minutes: self.daily_limit_minutes,
            break_minutes: self.break_minutes,
            playback_only: self.playback_only,
            enabled: self.usage_disclosure_accepted && self.service_disclosure_accepted,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Home,
    Records,
    Settings,
    SelectApps,
    Disclosure,
    Break,
}

pub struct ControlApp {
    #[cfg(all(
        target_os = "android",
        any(feature = "ads-test", feature = "ads-production")
    ))]
    ads_ui: Option<crate::ads::AdsUi>,
    #[cfg(all(
        target_os = "android",
        any(feature = "ads-test", feature = "ads-production")
    ))]
    ad_bounds: Option<[i32; 4]>,
    #[cfg(all(
        target_os = "android",
        any(feature = "ads-test", feature = "ads-production")
    ))]
    ad_pending: bool,
    saved: SavedState,
    #[cfg(not(target_os = "android"))]
    mode: AppMode,
    last_persisted: Option<SavedState>,
    last_persist_at: Instant,
    #[cfg(target_os = "android")]
    last_fallback_refresh: Instant,
    state_store: Option<crate::persistence::StateStore>,
    storage_error: Option<String>,
    last_state_reload: Instant,
    screen: Screen,
    permission_granted: bool,
    measurement_available: bool,
    usage_by_package_ms: HashMap<String, u64>,
    today_label: String,
    available_apps: Vec<TrackedApp>,
    apps_loading: bool,
    usage_loading: bool,
    app_filter: String,
    notice: Option<String>,
    guard_notice: Option<String>,
    last_refresh: Instant,
    #[cfg(target_os = "android")]
    events_tx: Sender<PlatformEvent>,
    #[cfg(target_os = "android")]
    events_rx: Receiver<PlatformEvent>,
    #[cfg(target_os = "android")]
    android_app: Option<android_activity::AndroidApp>,
    #[cfg(target_os = "android")]
    insets_loading: bool,
    #[cfg(target_os = "android")]
    window_insets: Option<[i32; 4]>,
    #[cfg(target_os = "android")]
    last_inset_refresh: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppMode {
    Runtime,
    #[cfg(not(target_os = "android"))]
    Preview,
}

#[cfg(not(target_os = "android"))]
impl AppMode {
    fn prepare_saved_state(self, mut state: SavedState) -> SavedState {
        if self == Self::Runtime {
            state.remove_preview_session();
        }
        state
    }
}

impl ControlApp {
    pub fn new(
        creation_context: &eframe::CreationContext<'_>,
        #[cfg_attr(not(target_os = "android"), allow(unused_variables))]
        #[cfg(target_os = "android")]
        android_app: Option<android_activity::AndroidApp>,
        #[cfg(not(target_os = "android"))] android_app: Option<()>,
    ) -> Self {
        Self::new_in_mode(creation_context, android_app, AppMode::Runtime)
    }

    #[cfg(not(target_os = "android"))]
    pub fn new_preview(creation_context: &eframe::CreationContext<'_>) -> Self {
        Self::new_in_mode(creation_context, None, AppMode::Preview)
    }

    fn new_in_mode(
        creation_context: &eframe::CreationContext<'_>,
        #[cfg(target_os = "android")] android_app: Option<android_activity::AndroidApp>,
        #[cfg(not(target_os = "android"))] _android_app: Option<()>,
        #[cfg_attr(target_os = "android", allow(unused_variables))] mode: AppMode,
    ) -> Self {
        configure_fonts_and_style(creation_context);

        let mut saved: SavedState = creation_context
            .storage
            .and_then(|storage| eframe::get_value(storage, SAVE_KEY))
            .unwrap_or_default();
        #[cfg(target_os = "android")]
        let mut state_store = android_app
            .as_ref()
            .and_then(|app| app.internal_data_path())
            .map(|path| crate::persistence::StateStore::new(path.join("usage-state.ron")));
        #[cfg(target_os = "ios")]
        let mut state_store = std::env::var_os("HOME")
            .filter(|home| std::path::Path::new(home).is_absolute())
            .map(|home| {
                crate::persistence::StateStore::new(std::path::PathBuf::from(home).join(
                    if mode == AppMode::Preview {
                        "Documents/preview-usage-state.ron"
                    } else {
                        "Documents/usage-state.ron"
                    },
                ))
            });
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let mut state_store: Option<crate::persistence::StateStore> = None;
        #[cfg(target_os = "ios")]
        let mut storage_error = if state_store.is_none() {
            Some("端末内の保存先を取得できないため保存を止めています。".to_owned())
        } else {
            None
        };
        #[cfg(not(target_os = "ios"))]
        let mut storage_error = None;
        if let Some(store) = &mut state_store {
            #[cfg(target_os = "android")]
            let restored = {
                crate::guard_storage::load_usage_transaction(store, || {
                    android_app
                        .as_ref()
                        .ok_or_else(|| std::io::Error::other("Android Activity unavailable"))
                        .and_then(crate::platform::storage_calendar)
                })
            };
            #[cfg(not(target_os = "android"))]
            let restored = store.load_and_migrate();
            match restored {
                Ok(Some(state)) => saved = state,
                Ok(None) => {}
                Err(_) => storage_error = Some("記録を読み込めないため保存を止めています。元の記録を保持して、もう一度読み込みます。".to_owned()),
            }
        }
        #[cfg(not(target_os = "android"))]
        let saved = mode.prepare_saved_state(saved);
        #[cfg(target_os = "android")]
        let (events_tx, events_rx) = mpsc::channel();
        #[cfg(target_os = "android")]
        if let Some(app) = &android_app
            && let Ok(day) = crate::platform::current_day(app)
        {
            saved.retain_current_day(&day);
        }
        let mut app = Self {
            #[cfg(all(
                target_os = "android",
                any(feature = "ads-test", feature = "ads-production")
            ))]
            ads_ui: android_app.as_ref().cloned().map(crate::ads::AdsUi::new),
            #[cfg(all(
                target_os = "android",
                any(feature = "ads-test", feature = "ads-production")
            ))]
            ad_bounds: None,
            #[cfg(all(
                target_os = "android",
                any(feature = "ads-test", feature = "ads-production")
            ))]
            ad_pending: false,
            last_persisted: None,
            last_persist_at: Instant::now(),
            #[cfg(target_os = "android")]
            last_fallback_refresh: Instant::now() - Duration::from_secs(120),
            state_store,
            storage_error,
            last_state_reload: Instant::now(),
            saved,
            #[cfg(not(target_os = "android"))]
            mode,
            screen: Screen::Home,
            permission_granted: false,
            measurement_available: false,
            usage_by_package_ms: HashMap::new(),
            today_label: "今日".to_owned(),
            available_apps: Vec::new(),
            apps_loading: false,
            usage_loading: false,
            app_filter: String::new(),
            notice: None,
            guard_notice: None,
            last_refresh: Instant::now() - Duration::from_secs(120),
            #[cfg(target_os = "android")]
            events_tx,
            #[cfg(target_os = "android")]
            events_rx,
            #[cfg(target_os = "android")]
            android_app,
            #[cfg(target_os = "android")]
            insets_loading: false,
            #[cfg(target_os = "android")]
            window_insets: None,
            #[cfg(target_os = "android")]
            last_inset_refresh: Instant::now() - Duration::from_secs(120),
        };

        #[cfg(target_os = "android")]
        {
            app.refresh_apps(creation_context.egui_ctx.clone());
            app.refresh_usage(creation_context.egui_ctx.clone());
        }

        #[cfg(not(target_os = "android"))]
        if mode == AppMode::Preview {
            app.load_preview_data();
        }

        app
    }

    #[cfg(not(target_os = "android"))]
    fn load_preview_data(&mut self) {
        if self.mode != AppMode::Preview {
            return;
        }
        self.permission_granted = true;
        self.measurement_available = true;
        self.today_label = "日".to_owned();
        if self.saved.target_apps.is_empty() {
            self.saved.target_apps = vec![
                TrackedApp {
                    label: "動画アプリ（プレビュー）".to_owned(),
                    package: "preview.video".to_owned(),
                },
                TrackedApp {
                    label: "SNS（プレビュー）".to_owned(),
                    package: "preview.social".to_owned(),
                },
            ];
        }
        if self.usage_by_package_ms.is_empty() {
            self.usage_by_package_ms
                .insert("preview.video".to_owned(), 102 * 60_000);
            self.usage_by_package_ms
                .insert("preview.social".to_owned(), 0);
        }
        self.saved.current_day_key = "preview".to_owned();
        if self.saved.daily_records.is_empty() {
            self.saved.daily_records = [
                ("月", 205),
                ("火", 142),
                ("水", 96),
                ("木", 82),
                ("金", 71),
                ("土", 126),
                ("日", 102),
            ]
            .into_iter()
            .map(|(label, minutes)| DailyRecord {
                day_key: label.to_owned(),
                label: label.to_owned(),
                used_ms: minutes * 60_000,
                playback_only: false,
            })
            .collect();
        }
    }

    fn drain_platform_events(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "android")]
        loop {
            match self.events_rx.try_recv() {
                Ok(PlatformEvent::InsetsLoaded(result)) => {
                    self.insets_loading = false;
                    if let Ok(insets) = result {
                        self.window_insets = insets;
                    }
                }
                Ok(PlatformEvent::AppsLoaded(result)) => {
                    self.apps_loading = false;
                    match result {
                        Ok(apps) => {
                            self.available_apps = apps;
                            self.notice = None;
                        }
                        Err(error) => self.notice = Some(error),
                    }
                }
                Ok(PlatformEvent::UsageLoaded {
                    selected_packages,
                    result,
                }) => {
                    self.usage_loading = false;
                    let current: Vec<_> = self
                        .saved
                        .target_apps
                        .iter()
                        .map(|app| app.package.clone())
                        .collect();
                    if selected_packages != current {
                        self.last_refresh = Instant::now() - Duration::from_secs(120);
                        continue;
                    }
                    match result {
                        Ok(snapshot) => self.apply_usage_snapshot(snapshot),
                        Err(error) => self.notice = Some(error),
                    }
                }
                Ok(PlatformEvent::ActionDone { label, result }) => match result {
                    Ok(()) => {
                        self.notice =
                            Some(format!("{label}を開きました。戻ると状態を更新します。"));
                        self.last_refresh = Instant::now() - Duration::from_secs(120);
                    }
                    Err(error) => self.notice = Some(error),
                },
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }

        #[cfg(not(target_os = "android"))]
        let _ = ctx;

        let now_ms = now_epoch_ms();
        if self
            .saved
            .cooldown_until_ms
            .is_some_and(|until| until <= now_ms)
        {
            self.saved.cooldown_until_ms = None;
            self.screen = Screen::Home;
            self.notice = Some("休憩時間が終わりました。アプリをもう一度開けます。".to_owned());
        }

        let refresh_seconds = if self.saved.service_disclosure_accepted {
            1
        } else {
            60
        };
        if self.last_refresh.elapsed() >= Duration::from_secs(refresh_seconds) {
            self.refresh_usage(ctx.clone());
        }
        self.sync_break_screen();
    }

    #[cfg(any(target_os = "android", test))]
    fn apply_usage_snapshot(&mut self, snapshot: UsageSnapshot) {
        self.accept_calendar_day(&snapshot.day_key);
        self.permission_granted = snapshot.permission_granted;
        self.measurement_available = snapshot.permission_granted && snapshot.measurement_available;
        self.today_label = snapshot.day_label.clone();
        if !snapshot.permission_granted || !snapshot.measurement_available {
            // Losing access does not turn an already recorded day into zero usage.
            return;
        }
        self.usage_by_package_ms = snapshot.usage_by_package_ms;

        if !self.saved.current_day_key.is_empty()
            && self.saved.current_day_key != snapshot.day_key
            && self.saved.limit_triggered_day.as_deref() != Some(snapshot.day_key.as_str())
        {
            // A break started before midnight still has to run for its full duration.
            self.saved.limit_triggered_day = None;
        }
        self.saved.current_day_key = snapshot.day_key.clone();

        let used_ms = self.today_usage_ms();
        if let Some(record) = self.saved.daily_records.iter_mut().find(|record| {
            record.day_key == snapshot.day_key && record.playback_only == self.saved.playback_only
        }) {
            record.label = snapshot.day_label;
            // OS event history can be pruned/reset on reboot. A successful read
            // must not erase the time already observed for this day.
            record.used_ms = record.used_ms.max(used_ms);
        } else {
            self.saved.daily_records.push(DailyRecord {
                day_key: snapshot.day_key.clone(),
                label: snapshot.day_label,
                used_ms,
                playback_only: self.saved.playback_only,
            });
        }
        self.saved
            .daily_records
            .retain(|record| record.day_key == snapshot.day_key);

        let limit_ms = u64::from(self.saved.daily_limit_minutes) * 60_000;
        if snapshot.permission_granted
            && used_ms >= limit_ms
            && self.saved.cooldown_until_ms.is_none()
            && self.saved.limit_triggered_day.as_deref() != Some(snapshot.day_key.as_str())
            && self
                .saved
                .last_limit_triggered_day
                .as_deref()
                .is_none_or(|day| snapshot.day_key.as_str() > day)
        {
            self.begin_break(snapshot.now_ms, snapshot.day_key);
        }
    }

    #[cfg(any(target_os = "android", test))]
    fn accept_calendar_day(&mut self, day: &str) {
        if !day.is_empty() && self.saved.current_day_key != day {
            self.saved.retain_current_day(day);
            self.usage_by_package_ms.clear();
            self.measurement_available = false;
            self.last_refresh = Instant::now() - Duration::from_secs(120);
        }
    }

    fn begin_break(&mut self, now_ms: i64, day_key: String) {
        self.saved.limit_triggered_day = Some(day_key.clone());
        if self
            .saved
            .last_limit_triggered_day
            .as_deref()
            .is_none_or(|day| day_key.as_str() > day)
        {
            self.saved.last_limit_triggered_day = Some(day_key);
        }
        self.saved.cooldown_until_ms =
            Some(now_ms + i64::from(self.saved.break_minutes.max(1)) * 60_000);
        self.screen = Screen::Break;
        self.notice = None;
    }

    fn sync_break_screen(&mut self) {
        if self.saved.cooldown_until_ms.is_some() {
            self.screen = Screen::Break;
        } else if self.screen == Screen::Break {
            self.screen = Screen::Home;
        }
    }

    fn today_usage_ms(&self) -> u64 {
        self.saved
            .target_apps
            .iter()
            .map(|app| {
                self.usage_by_package_ms
                    .get(&app.package)
                    .copied()
                    .unwrap_or_default()
            })
            .fold(0_u64, u64::saturating_add)
    }

    fn display_usage(&self) -> String {
        if !self.measurement_available {
            "取得できません".into()
        } else {
            format_usage(self.today_usage_ms())
        }
    }

    fn break_remaining_ms(&self) -> Option<i64> {
        self.saved
            .cooldown_until_ms
            .map(|until| until.saturating_sub(now_epoch_ms()).max(0))
    }

    fn persist_changed_state(&mut self, frame: &mut eframe::Frame) {
        #[cfg(target_os = "android")]
        let calendar_day = self
            .android_app
            .as_ref()
            .and_then(|app| crate::platform::current_day(app).ok());
        #[cfg(target_os = "android")]
        if let Some(day) = &calendar_day {
            self.accept_calendar_day(day);
        }
        #[cfg(target_os = "ios")]
        if self.state_store.is_none() {
            return;
        }
        if self.storage_error.is_none() && self.last_persisted.as_ref() == Some(&self.saved) {
            return;
        }
        if self.storage_error.is_none()
            && let Some(previous) = &self.last_persisted
            && previous.target_apps == self.saved.target_apps
            && previous.daily_limit_minutes == self.saved.daily_limit_minutes
            && previous.break_minutes == self.saved.break_minutes
            && previous.cooldown_until_ms == self.saved.cooldown_until_ms
            && previous.limit_triggered_day == self.saved.limit_triggered_day
            && previous.last_limit_triggered_day == self.saved.last_limit_triggered_day
            && previous.current_day_key == self.saved.current_day_key
            && previous.usage_disclosure_accepted == self.saved.usage_disclosure_accepted
            && previous.service_disclosure_accepted == self.saved.service_disclosure_accepted
            && previous.playback_only == self.saved.playback_only
            && self.last_persist_at.elapsed() < Duration::from_secs(10)
        {
            return;
        }
        if let Some(store) = &self.state_store {
            if !store.is_ready() {
                return;
            }
            #[cfg(target_os = "android")]
            let result = self
                .android_app
                .as_ref()
                .and_then(|app| app.internal_data_path())
                .ok_or_else(|| std::io::Error::other("Missing Android state directory"))
                .and_then(|dir| {
                    crate::guard_storage::save_usage_transaction(store, &dir, &self.saved, || {
                        self.android_app
                            .as_ref()
                            .ok_or_else(|| std::io::Error::other("Android Activity unavailable"))
                            .and_then(crate::platform::storage_calendar)
                    })
                });
            #[cfg(not(target_os = "android"))]
            let result = store.save(&self.saved).map(|_| self.saved.clone());
            if self.accept_persistence_result(result) {
                #[cfg(target_os = "android")]
                if let Some(storage) = frame.storage_mut() {
                    // The canonical state has been saved. Do not leave the pre-migration
                    // app record in eframe's independent preferences file.
                    storage.remove_string(SAVE_KEY);
                    storage.flush();
                }
            }
        } else if let Some(storage) = frame.storage_mut() {
            eframe::set_value(storage, SAVE_KEY, &self.saved);
            storage.flush();
            self.last_persisted = Some(self.saved.clone());
            self.last_persist_at = Instant::now();
        }
    }

    fn restore_saved_state(&mut self, state: SavedState) {
        #[cfg(not(target_os = "android"))]
        let state = self.mode.prepare_saved_state(state);
        self.screen = if state
            .cooldown_until_ms
            .is_some_and(|until| until > now_epoch_ms())
        {
            Screen::Break
        } else {
            Screen::Home
        };
        // A reloaded record is not a fresh measurement, even for the same day.
        self.usage_by_package_ms.clear();
        self.measurement_available = false;
        self.saved = state;
        self.last_persisted = None;
        self.last_refresh = Instant::now() - Duration::from_secs(120);
    }

    fn accept_persisted_state(&mut self, state: SavedState) {
        #[cfg(any(target_os = "android", test))]
        self.accept_calendar_day(&state.current_day_key);
        self.saved = state;
        self.sync_break_screen();
        self.last_persisted = Some(self.saved.clone());
        self.last_persist_at = Instant::now();
        self.storage_error = None;
    }

    fn accept_persistence_result(&mut self, result: std::io::Result<SavedState>) -> bool {
        match result {
            Ok(saved) => {
                self.accept_persisted_state(saved);
                true
            }
            Err(_) => {
                self.storage_error = Some(
                    "設定と休憩状態を保存できません。端末の空き容量を確認してください。".into(),
                );
                false
            }
        }
    }

    fn refresh_apps(&mut self, ctx: egui::Context) {
        #[cfg(target_os = "android")]
        if let Some(android_app) = &self.android_app {
            self.apps_loading = true;
            crate::platform::request_apps(android_app.clone(), self.events_tx.clone(), ctx);
        }
        #[cfg(not(target_os = "android"))]
        let _ = ctx;
    }

    fn refresh_usage(&mut self, ctx: egui::Context) {
        self.last_refresh = Instant::now();
        if self.usage_loading || !self.saved.usage_disclosure_accepted {
            return;
        }
        #[cfg(target_os = "android")]
        if self.saved.service_disclosure_accepted {
            if let Some(status) = crate::service::latest_status()
                && status.policy == self.saved.policy()
                && (0..=6_000).contains(&now_epoch_ms().saturating_sub(status.snapshot.now_ms))
            {
                self.guard_notice = None;
                self.saved.accept_service_break(
                    status.cooldown_until_ms,
                    status.triggered_day,
                    status.last_triggered_day,
                    now_epoch_ms(),
                );
                if status.storage_error {
                    self.guard_notice = Some("状態を読み書きできません。前の記録を保持し、確認できた利用分の制限は続けています。".into());
                }
                if self.saved.playback_only && !status.media_available && !status.storage_error {
                    self.guard_notice = Some("選んだアプリの再生状態を取得できません。通知へのアクセスの許可と、対象アプリが再生状態を通知するかを確認してください。前の記録を保持しています。".into());
                }
                self.apply_usage_snapshot(status.snapshot);
                return;
            }
            if crate::service::service_running() {
                self.guard_notice = Some(
                    "時間の計測を更新しています。取得が終わるまで前の記録を保持します。".into(),
                );
                self.measurement_available = false;
                return;
            }
            self.guard_notice = Some("使用中の中断はまだ動作を確認できません。Androidのユーザー補助で利用制限を有効にしてください。".into());
            if self.saved.playback_only {
                self.measurement_available = false;
                return;
            }
            if self.last_fallback_refresh.elapsed() < Duration::from_secs(60) {
                return;
            }
            self.last_fallback_refresh = Instant::now();
        }
        #[cfg(target_os = "android")]
        if let Some(android_app) = &self.android_app {
            self.usage_loading = true;
            let selected_packages = self
                .saved
                .target_apps
                .iter()
                .map(|app| app.package.clone())
                .collect();
            crate::platform::request_usage(
                android_app.clone(),
                selected_packages,
                self.events_tx.clone(),
                ctx,
            );
        }
        #[cfg(not(target_os = "android"))]
        let _ = ctx;
    }

    fn request_usage_settings(&mut self, ctx: egui::Context) {
        self.saved.usage_disclosure_accepted = true;
        self.screen = Screen::Home;
        self.notice = None;
        #[cfg(target_os = "android")]
        if let Some(android_app) = &self.android_app {
            crate::platform::open_usage_settings(android_app.clone(), self.events_tx.clone(), ctx);
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = ctx;
            if self.mode == AppMode::Preview {
                self.permission_granted = true;
                self.notice =
                    Some("プレビューでは使用状況へのアクセスを許可した状態です。".to_owned());
            }
        }
    }

    fn request_home_settings(&mut self, ctx: egui::Context) {
        #[cfg(target_os = "android")]
        if let Some(android_app) = &self.android_app {
            crate::platform::open_home_settings(android_app.clone(), self.events_tx.clone(), ctx);
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = ctx;
            self.notice = Some("Androidではホーム画面の選択を開きます。".to_owned());
        }
    }

    fn request_guard_settings(&mut self, ctx: egui::Context, playback: bool) {
        if !self.saved.usage_disclosure_accepted {
            self.screen = Screen::Disclosure;
            return;
        }
        self.saved.service_disclosure_accepted = true;
        #[cfg(target_os = "android")]
        if let Some(android_app) = &self.android_app {
            crate::platform::open_guard_settings(
                android_app.clone(),
                playback,
                self.events_tx.clone(),
                ctx,
            );
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = (ctx, playback);
            self.notice = Some("Androidでは説明を確認した後に、ユーザー補助または通知へのアクセスの設定を開きます。".into());
        }
    }

    fn return_to_device_home(&mut self, ctx: egui::Context) {
        #[cfg(target_os = "android")]
        if let Some(android_app) = &self.android_app {
            crate::platform::return_to_home(android_app.clone(), self.events_tx.clone(), ctx);
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = ctx;
            self.screen = Screen::Home;
        }
    }

    fn toggle_app(&mut self, app: &TrackedApp, ctx: &egui::Context) {
        if let Some(index) = self
            .saved
            .target_apps
            .iter()
            .position(|selected| selected.package == app.package)
        {
            self.saved.target_apps.remove(index);
            self.usage_by_package_ms.remove(&app.package);
        } else {
            self.saved.target_apps.push(app.clone());
        }
        self.saved
            .target_apps
            .sort_by_key(|app| app.label.to_lowercase());
        // An aggregate from the previous selection is no longer this selection's total.
        self.saved.daily_records.clear();
        self.refresh_usage(ctx.clone());
    }

    fn open_target_app(&mut self, target: TrackedApp, ctx: egui::Context) {
        if self.saved.cooldown_until_ms.is_some() {
            self.screen = Screen::Break;
            return;
        }

        if self.permission_granted
            && self.today_usage_ms() >= u64::from(self.saved.daily_limit_minutes) * 60_000
            && self.saved.limit_triggered_day.as_deref()
                != Some(self.saved.current_day_key.as_str())
            && self
                .saved
                .last_limit_triggered_day
                .as_deref()
                .is_none_or(|day| self.saved.current_day_key.as_str() > day)
        {
            self.begin_break(now_epoch_ms(), self.saved.current_day_key.clone());
            return;
        }

        self.notice = None;
        #[cfg(target_os = "android")]
        if let Some(android_app) = &self.android_app {
            crate::platform::launch_package(
                android_app.clone(),
                target.package,
                target.label,
                self.events_tx.clone(),
                ctx,
            );
            return;
        }

        self.notice = Some(format!("プレビュー: 「{}」を開く操作です。", target.label));
        self.screen = Screen::Home;
        #[cfg(not(target_os = "android"))]
        let _ = ctx;
    }

    fn draw_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("利用制限").size(28.0).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(RichText::new("⚙").size(24.0)).clicked() {
                    self.screen = Screen::Settings;
                }
            });
        });
        ui.add_space(8.0);
    }

    fn draw_home(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if !self.permission_granted {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(RichText::new("利用時間の記録がオフです").strong());
                ui.label("選んだアプリの前面利用時間を端末内で調べます。");
                if ui.button("説明を読んで設定する").clicked() {
                    self.screen = Screen::Disclosure;
                }
            });
            ui.add_space(10.0);
        }

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("▶").size(24.0).color(ORANGE));
                ui.label(RichText::new("対象アプリ").size(18.0).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("変更 ›").clicked() {
                        self.screen = Screen::SelectApps;
                        if self.available_apps.is_empty() && !self.apps_loading {
                            self.refresh_apps(ctx.clone());
                        }
                    }
                });
            });
            if self.saved.target_apps.is_empty() {
                ui.label("制限するアプリを選んでください。");
                if ui.button("アプリを選ぶ").clicked() {
                    self.screen = Screen::SelectApps;
                    if self.available_apps.is_empty() && !self.apps_loading {
                        self.refresh_apps(ctx.clone());
                    }
                }
            } else {
                for target in self.saved.target_apps.clone() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&target.label).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let used = self
                                .usage_by_package_ms
                                .get(&target.package)
                                .copied()
                                .unwrap_or_default();
                            if ui
                                .button(format!(
                                    "開く  ›  {}",
                                    if self.measurement_available {
                                        format_usage(used)
                                    } else {
                                        "未取得".into()
                                    }
                                ))
                                .clicked()
                            {
                                self.open_target_app(target.clone(), ctx.clone());
                            }
                        });
                    });
                }
            }
        });

        ui.add_space(10.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("1日の利用上限").size(16.0));
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format_limit(self.saved.daily_limit_minutes))
                        .size(30.0)
                        .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new("上限")
                            .size(14.0)
                            .color(if ui.visuals().dark_mode {
                                Color32::from_gray(180)
                            } else {
                                MUTED
                            }),
                    );
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if self.saved.playback_only {
                        "今日の再生時間"
                    } else {
                        "今日の利用時間"
                    })
                    .color(if ui.visuals().dark_mode {
                        Color32::from_gray(180)
                    } else {
                        MUTED
                    }),
                );
                if self.usage_loading {
                    ui.spinner();
                }
            });
            ui.label(RichText::new(self.display_usage()).size(31.0).strong());
            let limit_ms = u64::from(self.saved.daily_limit_minutes) * 60_000;
            let fraction = if limit_ms == 0 {
                0.0
            } else {
                self.today_usage_ms() as f32 / limit_ms as f32
            };
            ui.add(
                egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                    .desired_width(ui.available_width())
                    .fill(ORANGE),
            );
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format_limit(self.saved.daily_limit_minutes)).color(
                            if ui.visuals().dark_mode {
                                Color32::from_gray(180)
                            } else {
                                MUTED
                            },
                        ),
                    );
                });
            });
        });

        ui.add_space(10.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("☕").size(20.0));
                ui.label(
                    RichText::new(format!("上限後は{}分休憩", self.saved.break_minutes)).strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("変更 ›").clicked() {
                        self.screen = Screen::Settings;
                    }
                });
            });
        });

        ui.add_space(10.0);
        egui::Frame::new()
            .fill(if ui.visuals().dark_mode {
                Color32::from_rgb(48, 48, 50)
            } else {
                Color32::from_rgb(235, 232, 227)
            })
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.label(
                    RichText::new("使いすぎを防いで、やりたいことに集中できる毎日へ。").color(
                        if ui.visuals().dark_mode {
                            Color32::from_gray(180)
                        } else {
                            INK
                        },
                    ),
                );
            });

        ui.add_space(10.0);
        egui::Frame::new()
            .fill(if ui.visuals().dark_mode { Color32::from_rgb(48, 48, 50) } else { Color32::from_rgb(225, 220, 211) })
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                #[cfg(all(target_os = "android", any(feature = "ads-test", feature = "ads-production")))]
                if let Some(ads) = &self.ads_ui {
                    ui.label(RichText::new(if cfg!(feature = "ads-production") { "広告" } else { "広告（試験用）" }).strong());
                    ui.label("表示を選ぶとGoogleへ端末情報などを通信します。選んだアプリ名と利用時間は渡しません。");
                    ui.horizontal(|ui| {
                        if ads.is_requested() {
                            if ui.button("広告を閉じる").clicked() { ads.close(); }
                        } else if ui.button(if cfg!(feature = "ads-production") { "任意の広告を表示" } else { "任意の試験広告を表示" }).clicked() { self.ad_pending = true; }
                        if ui.button("広告の同意設定").clicked() { ads.privacy(); }
                    });
                    ui.label(ads.status());
                    let (slot, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 65.0), egui::Sense::hover());
                    if ui.clip_rect().contains_rect(slot) {
                        let scale = ctx.pixels_per_point();
                        self.ad_bounds = Some([(slot.left()*scale).round() as i32, (slot.top()*scale).round() as i32, (slot.width()*scale).floor() as i32, (slot.height()*scale).floor() as i32]);
                        if self.ad_pending {
                            self.ad_pending = false;
                            ads.request();
                        }
                    } else if self.ad_pending {
                        // 広告欄が画面外のまま読み込むと直ちに閉じられるため、先に見える位置へ動かす。
                        ui.scroll_to_rect(slot.expand(16.0), Some(Align::Center));
                        ctx.request_repaint();
                    }
                    ui.label(RichText::new("広告を閉じても休憩や制限は変わりません。").size(12.0).color(if ui.visuals().dark_mode { Color32::from_gray(180) } else { MUTED }));
                }
                #[cfg(not(all(target_os = "android", any(feature = "ads-test", feature = "ads-production"))))]
                {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new("広告").strong());
                        ui.label(RichText::new("任意表示の枠（配信準備中）").color(if ui.visuals().dark_mode { Color32::from_gray(180) } else { MUTED }));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("›").size(24.0).color(ORANGE));
                    });
                });
                ui.label(
                    RichText::new("休憩時間の解除条件には使いません。")
                        .size(12.0)
                        .color(if ui.visuals().dark_mode { Color32::from_gray(180) } else { MUTED }),
                );
                }
            });

        self.show_notice(ui);
    }

    fn draw_records(&mut self, ui: &mut egui::Ui) {
        ui.heading("今日の記録");
        ui.add_space(8.0);

        egui::Frame::new()
            .fill(if ui.visuals().dark_mode {
                Color32::from_rgb(48, 48, 50)
            } else {
                Color32::from_rgb(252, 235, 222)
            })
            .inner_margin(egui::Margin::same(15))
            .show(ui, |ui| {
                ui.label(format!(
                    "{}の{}",
                    self.today_label,
                    if self.saved.playback_only {
                        "再生時間"
                    } else {
                        "利用時間"
                    }
                ));
                ui.label(
                    RichText::new(self.display_usage())
                        .size(30.0)
                        .strong()
                        .color(ORANGE),
                );
                ui.label("今日の合計利用時間");
            });

        ui.add_space(10.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("1週間の利用時間").strong());
            let records: Vec<DailyRecord> = self
                .saved
                .daily_records
                .iter()
                .filter(|record| record.playback_only == self.saved.playback_only)
                .rev()
                .take(7)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let max_ms = records
                .iter()
                .map(|record| record.used_ms)
                .max()
                .unwrap_or(60_000)
                .max(60_000);
            if records.is_empty() {
                ui.label("記録はまだありません。");
            }
            for record in records {
                ui.horizontal(|ui| {
                    ui.add_sized([54.0, 20.0], egui::Label::new(&record.label));
                    let fill = if record.day_key == self.saved.current_day_key {
                        ORANGE
                    } else {
                        Color32::from_rgb(154, 158, 160)
                    };
                    ui.add(
                        egui::ProgressBar::new(record.used_ms as f32 / max_ms as f32)
                            .desired_width(ui.available_width() - 76.0)
                            .fill(fill),
                    );
                    ui.label(format_usage(record.used_ms));
                });
            }
        });

        ui.add_space(10.0);
        egui::Frame::new()
            .fill(if ui.visuals().dark_mode {
                Color32::from_rgb(48, 48, 50)
            } else {
                Color32::from_rgb(225, 220, 211)
            })
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("広告").strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("任意表示・配信準備中").color(
                            if ui.visuals().dark_mode {
                                Color32::from_gray(180)
                            } else {
                                MUTED
                            },
                        ));
                    });
                });
            });

        ui.add_space(10.0);
        ui.label("記録はこの端末内に保存します。アプリ名や利用時間を外部へ送信しません。");
        self.show_notice(ui);
    }

    fn draw_settings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("設定");
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("1日の利用上限").strong());
            ui.add(
                egui::Slider::new(&mut self.saved.daily_limit_minutes, 30..=360)
                    .step_by(30.0)
                    .suffix("分"),
            );
            ui.label(
                RichText::new(format_limit(self.saved.daily_limit_minutes))
                    .size(20.0)
                    .strong(),
            );

            ui.add_space(8.0);
            ui.label(RichText::new("上限後の休憩時間").strong());
            ui.add(
                egui::Slider::new(&mut self.saved.break_minutes, 5..=120)
                    .step_by(5.0)
                    .suffix("分"),
            );
            ui.label(
                RichText::new(format!("{}分", self.saved.break_minutes))
                    .size(20.0)
                    .strong(),
            );
        });

        ui.add_space(10.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("利用時間へのアクセス").strong());
            ui.label("選んだアプリの前面利用時間を読み取り、端末内だけに記録します。");
            ui.label(if self.permission_granted {
                "現在: 許可されています"
            } else {
                "現在: 未許可です。制限は動作しません。"
            });
            if ui.button("説明を確認して設定を開く").clicked() {
                self.screen = Screen::Disclosure;
            }
        });

        ui.add_space(10.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("ホーム画面に設定").strong());
            ui.label(
                "ホームボタンからこのアプリに戻ると、利用時間を確認して休憩画面を表示します。",
            );
            ui.label("ホーム画面の設定だけでは、通知や外部リンクからの起動は制御しません。使用中の中断には、下のユーザー補助の許可が必要です。");
            if ui.button("Androidのホーム画面を選ぶ").clicked() {
                self.request_home_settings(ctx.clone());
            }
        });

        ui.add_space(10.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("使用中の中断と再生時間").strong());
            ui.label("ユーザー補助を許可すると、選んだアプリの上限を確認して休憩画面へ切り替えます。画面の文字・画像・操作内容は読みません。許可はAndroid設定で解除できます。");
            if ui.button("説明を確認してユーザー補助を開く").clicked() {
                self.request_guard_settings(ctx.clone(), false);
            }
            if ui.checkbox(&mut self.saved.playback_only, "再生中と通知された時間だけを数える").changed() {
                self.measurement_available = false;
                self.last_refresh = Instant::now() - Duration::from_secs(120);
            }
            ui.label("再生時間には通知へのアクセスも必要です。通知本文は読みません。対象が前面にあり再生中の場合だけ数え、停止・読み込み待ち・画面消灯を除きます。休憩中は、選んだアプリの再生へ一時停止を要求します。対象が要求に応じない場合は止められません。動画と音声の区別や、画面を見ていたことは判定できません。再生状態を通知しないアプリには対応しません。");
            if ui.button("説明を確認して通知へのアクセスを開く").clicked() {
                self.request_guard_settings(ctx.clone(), true);
            }
        });

        ui.add_space(10.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("広告").strong());
            ui.label("広告はこのアプリ内だけに表示する想定です。休憩画面には出さず、広告を断っても制限機能は変わりません。");
            #[cfg(all(target_os = "android", feature = "ads-test"))]
            ui.label("この試験版はGoogleの試験広告に接続します。ホームで広告表示を選ぶと同意を確認して広告を読み込みます。「広告の同意設定」を選んだ場合もGoogleへ接続して同意情報を確認します。");
            #[cfg(all(target_os = "android", feature = "ads-production"))]
            ui.label("ホームで広告表示を選ぶと同意を確認して広告を読み込みます。「広告の同意設定」を選んだ場合もGoogleへ接続して同意情報を確認します。");
            #[cfg(not(all(target_os = "android", any(feature = "ads-test", feature = "ads-production"))))]
            ui.label("この版は広告枠だけで、広告配信には接続しません。");
        });

        self.show_notice(ui);
    }

    fn draw_app_selector(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            if ui.button("‹ 戻る").clicked() {
                self.screen = Screen::Home;
            }
            ui.heading("対象アプリを選ぶ");
        });
        ui.label("この端末でホーム画面から起動できるアプリを表示します。");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("検索");
            ui.text_edit_singleline(&mut self.app_filter);
        });

        if self.apps_loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("アプリ一覧を読み込んでいます…");
            });
        }
        if self.available_apps.is_empty() && !self.apps_loading {
            ui.label("起動できるアプリが見つかりませんでした。");
            if ui.button("一覧を再読み込み").clicked() {
                self.refresh_apps(ctx.clone());
            }
        }

        let filter = self.app_filter.trim().to_lowercase();
        let apps: Vec<TrackedApp> = self
            .available_apps
            .iter()
            .filter(|app| app.label.to_lowercase().contains(&filter))
            .cloned()
            .collect();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for app in apps {
                let mut selected = self
                    .saved
                    .target_apps
                    .iter()
                    .any(|target| target.package == app.package);
                let response = ui.checkbox(&mut selected, RichText::new(&app.label).size(17.0));
                if response.changed() {
                    self.toggle_app(&app, ctx);
                }
                ui.add_space(4.0);
            }
        });

        ui.add_space(6.0);
        if ui.button("選択を保存して戻る").clicked() {
            self.screen = Screen::Home;
            self.refresh_usage(ctx.clone());
        }
        self.show_notice(ui);
    }

    fn draw_disclosure(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("利用時間の読み取りについて");
        ui.add_space(10.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                "このアプリは、あなたが選んだアプリの前面利用時間を読み取り、設定した上限と休憩時間を動かします。",
            );
            ui.add_space(8.0);
            ui.label("利用時間と対象アプリ名はこの端末内に保存します。外部サーバーや広告事業者には送りません。");
            ui.add_space(8.0);
            ui.label("許可は端末設定からいつでも解除できます。許可しない場合、時間の記録と休憩制御は動きません。");
        });
        ui.add_space(12.0);
        if full_width_button(ui, "内容を確認して使用状況を設定", ORANGE) {
            self.request_usage_settings(ctx.clone());
        }
        if ui.button("今は設定しない").clicked() {
            self.screen = Screen::Settings;
        }
        self.show_notice(ui);
    }

    fn draw_break(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(error) = &self.storage_error {
            ui.colored_label(ORANGE, error);
        }
        let remaining = self.break_remaining_ms().unwrap_or_default();
        let duration = Duration::from_millis(remaining as u64);
        let total_seconds = duration.as_secs();
        let countdown = format!("{:02}:{:02}", total_seconds / 60, total_seconds % 60);
        ui.with_layout(Layout::top_down(Align::Center), |ui| {
            ui.add_space((ui.available_height() * 0.08).max(24.0));
            ui.label(RichText::new("Ⅱ").size(70.0).strong().color(ORANGE));
            ui.add_space(6.0);
            ui.label(
                RichText::new("休憩中")
                    .size(42.0)
                    .strong()
                    .color(Color32::WHITE),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("対象アプリは休止中")
                    .size(21.0)
                    .color(Color32::WHITE),
            );

            if !self.saved.target_apps.is_empty() {
                ui.add_space(8.0);
                let names = self
                    .saved
                    .target_apps
                    .iter()
                    .map(|app| app.label.as_str())
                    .collect::<Vec<_>>()
                    .join("・");
                ui.label(RichText::new(names).color(Color32::from_rgb(207, 207, 207)));
            }

            ui.add_space(25.0);
            egui::Frame::new()
                .fill(Color32::from_rgb(45, 45, 47))
                .inner_margin(egui::Margin::same(18))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("残り").size(22.0).color(Color32::WHITE));
                        ui.label(RichText::new(countdown).size(43.0).strong().color(ORANGE));
                    });
                });
            ui.add_space(18.0);
            ui.label(RichText::new("休憩が終わると、もう一度開けます").color(Color32::WHITE));

            ui.add_space(32.0);
            if full_width_button(ui, "ホームへ戻る", ORANGE) {
                self.return_to_device_home(ctx.clone());
            }
        });
    }

    fn show_notice(&self, ui: &mut egui::Ui) {
        if let Some(error) = &self.storage_error {
            ui.colored_label(ORANGE, error);
        }
        if let Some(notice) = &self.notice {
            ui.add_space(8.0);
            ui.colored_label(ORANGE, notice);
        }
        if let Some(notice) = &self.guard_notice {
            ui.add_space(8.0);
            ui.colored_label(ORANGE, notice);
        }
    }
}

fn configure_fonts_and_style(creation_context: &eframe::CreationContext<'_>) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "noto-sans-jp".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/NotoSansJP[wght].ttf"
        ))),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "noto-sans-jp".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "noto-sans-jp".to_owned());
    creation_context.egui_ctx.set_fonts(fonts);

    let mut style = (*creation_context.egui_ctx.style_of(egui::Theme::Light)).clone();
    style.visuals = egui::Visuals::light();
    style.visuals.override_text_color = Some(INK);
    style.spacing.item_spacing = egui::vec2(9.0, 10.0);
    creation_context
        .egui_ctx
        .set_style_of(egui::Theme::Light, style);
    creation_context.egui_ctx.set_theme(egui::Theme::Light);
}

fn full_width_button(ui: &mut egui::Ui, label: &str, fill: Color32) -> bool {
    let width = ui.available_width();
    ui.add_sized(
        [width, 56.0],
        egui::Button::new(RichText::new(label).strong().color(Color32::WHITE)).fill(fill),
    )
    .clicked()
}

fn format_limit(minutes: u32) -> String {
    if minutes >= 60 && minutes.is_multiple_of(60) {
        format!("{}時間", minutes / 60)
    } else if minutes >= 60 {
        format!("{}時間{}分", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}分")
    }
}

fn format_usage(milliseconds: u64) -> String {
    let total_minutes = milliseconds / 60_000;
    if total_minutes >= 60 {
        format!("{}時間{}分", total_minutes / 60, total_minutes % 60)
    } else {
        format!("{total_minutes}分")
    }
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

impl eframe::App for ControlApp {
    #[cfg(target_os = "android")]
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if (self.window_insets.is_none()
            || self.last_inset_refresh.elapsed() >= Duration::from_secs(1))
            && !self.insets_loading
            && let Some(app) = &self.android_app
        {
            self.insets_loading = true;
            self.last_inset_refresh = Instant::now();
            crate::platform::request_insets(app.clone(), self.events_tx.clone(), ctx.clone());
        }
        if let Some(app) = &self.android_app
            && let Some(window) = app.native_window()
        {
            let rect = app.content_rect();
            let width = window.width();
            let height = window.height();
            let insets = self.window_insets.unwrap_or_default();
            if rect.right > rect.left && rect.bottom > rect.top {
                let scale = input
                    .viewports
                    .get(&input.viewport_id)
                    .and_then(|info| info.native_pixels_per_point)
                    .unwrap_or(1.0)
                    * ctx.zoom_factor();
                input.safe_area_insets = Some(egui::SafeAreaInsets(egui::epaint::MarginF32 {
                    left: rect.left.max(insets[0]).max(0) as f32 / scale,
                    top: rect.top.max(insets[1]).max(0) as f32 / scale,
                    right: (width - rect.right).max(insets[2]).max(0) as f32 / scale,
                    bottom: (height - rect.bottom).max(insets[3]).max(0) as f32 / scale,
                }));
            }
        }
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "android")]
        if let Some(app) = &self.android_app {
            // Read current Resources: the native configuration can retain its initial value.
            let dark =
                crate::platform::system_dark_mode(app).unwrap_or(ctx.theme() == egui::Theme::Dark);
            ctx.set_theme(if dark {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            });
        }
        if self.last_state_reload.elapsed() >= Duration::from_secs(5)
            && let Some(store) = &mut self.state_store
            && !store.is_ready()
        {
            self.last_state_reload = Instant::now();
            #[cfg(target_os = "android")]
            let restored = {
                crate::guard_storage::load_usage_transaction(store, || {
                    self.android_app
                        .as_ref()
                        .ok_or_else(|| std::io::Error::other("Android Activity unavailable"))
                        .and_then(crate::platform::storage_calendar)
                })
            };
            #[cfg(not(target_os = "android"))]
            let restored = store.load_and_migrate();
            if let Ok(state) = restored {
                if let Some(state) = state {
                    self.restore_saved_state(state);
                }
                self.storage_error = None;
                self.last_persisted = None;
                self.last_refresh = Instant::now() - Duration::from_secs(120);
            }
        }
        self.drain_platform_events(ctx);
        #[cfg(target_os = "android")]
        if let Some(app) = &self.android_app
            && let Ok(day) = crate::platform::current_day(app)
        {
            self.accept_calendar_day(&day);
        }
    }

    fn ui(&mut self, root_ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        #[cfg(all(
            target_os = "android",
            any(feature = "ads-test", feature = "ads-production")
        ))]
        {
            self.ad_bounds = None;
        }
        let ctx = root_ui.ctx().clone();
        root_ui.set_style(ctx.style_of(ctx.theme()));
        let is_break = self.screen == Screen::Break;
        let background = if is_break || root_ui.visuals().dark_mode {
            DARK
        } else {
            PAPER
        };

        root_ui.scope_builder(
            egui::UiBuilder::new().max_rect(ctx.content_rect()),
            |root_ui| {
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(background)
                            .inner_margin(egui::Margin::same(17)),
                    )
                    .show(root_ui, |ui| {
                        if self.screen == Screen::Break {
                            self.draw_break(ui, &ctx);
                            return;
                        }

                        self.draw_header(ui);
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .max_height((ui.available_height() - 72.0).max(100.0))
                            .show(ui, |ui| match self.screen {
                                Screen::Home => self.draw_home(ui, &ctx),
                                Screen::Records => self.draw_records(ui),
                                Screen::Settings => self.draw_settings(ui, &ctx),
                                Screen::SelectApps => self.draw_app_selector(ui, &ctx),
                                Screen::Disclosure => self.draw_disclosure(ui, &ctx),
                                Screen::Break => {}
                            });

                        ui.add_space(6.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            let item_width = ui.available_width() / 3.0;
                            for (screen, label, symbol) in [
                                (Screen::Home, "ホーム", "○"),
                                (Screen::Records, "記録", "▥"),
                                (Screen::Settings, "設定", "●"),
                            ] {
                                let selected = self.screen == screen;
                                let color = if selected {
                                    ORANGE
                                } else {
                                    if ui.visuals().dark_mode {
                                        Color32::from_gray(180)
                                    } else {
                                        MUTED
                                    }
                                };
                                let response = ui.add_sized(
                                    [item_width, 48.0],
                                    egui::Button::new(
                                        RichText::new(format!("{symbol}\n{label}"))
                                            .size(13.0)
                                            .color(color),
                                    )
                                    .fill(Color32::TRANSPARENT)
                                    .stroke(Stroke::NONE),
                                );
                                if response.clicked() {
                                    self.screen = screen;
                                }
                            }
                        });
                    });
            },
        );

        #[cfg(all(
            target_os = "android",
            any(feature = "ads-test", feature = "ads-production")
        ))]
        if let Some(ads) = &self.ads_ui {
            if self.screen != Screen::Home || self.break_remaining_ms().is_some() {
                self.ad_pending = false;
            }
            ads.sync(
                self.screen == Screen::Home && self.break_remaining_ms().is_none(),
                self.ad_bounds,
            );
        }
        self.persist_changed_state(frame);
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        #[cfg(target_os = "android")]
        let calendar_day = self
            .android_app
            .as_ref()
            .and_then(|app| crate::platform::current_day(app).ok());
        #[cfg(target_os = "android")]
        if let Some(day) = &calendar_day {
            self.accept_calendar_day(day);
        }
        #[cfg(target_os = "ios")]
        if self.state_store.is_none() {
            return;
        }
        if let Some(store) = &self.state_store {
            #[cfg(target_os = "android")]
            let result = self
                .android_app
                .as_ref()
                .and_then(|app| app.internal_data_path())
                .ok_or_else(|| std::io::Error::other("Missing Android state directory"))
                .and_then(|dir| {
                    crate::guard_storage::save_usage_transaction(store, &dir, &self.saved, || {
                        self.android_app
                            .as_ref()
                            .ok_or_else(|| std::io::Error::other("Android Activity unavailable"))
                            .and_then(crate::platform::storage_calendar)
                    })
                });
            #[cfg(not(target_os = "android"))]
            let result = store.save(&self.saved).map(|_| self.saved.clone());
            if self.accept_persistence_result(result) {
                storage.remove_string(SAVE_KEY);
            }
        } else {
            eframe::set_value(storage, SAVE_KEY, &self.saved);
        }
    }
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;

    #[cfg(target_os = "ios")]
    static IOS_DOCUMENTS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct LegacyStorage(String);
    impl eframe::Storage for LegacyStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            (key == SAVE_KEY).then(|| self.0.clone())
        }
        fn set_string(&mut self, _: &str, _: String) {}
        fn remove_string(&mut self, _: &str) {}
        fn flush(&mut self) {}
    }

    fn construct_from_legacy(state: &SavedState) -> ControlApp {
        #[cfg(target_os = "ios")]
        let _documents_guard = IOS_DOCUMENTS_TEST_LOCK.lock().unwrap();
        let storage = LegacyStorage(ron::to_string(state).unwrap());
        let mut context = eframe::CreationContext::_new_kittest(egui::Context::default());
        context.storage = Some(&storage);
        ControlApp::new(&context, None)
    }

    #[cfg(target_os = "ios")]
    struct NativeDocumentsFixture {
        path: std::path::PathBuf,
        cleaned: bool,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    #[cfg(target_os = "ios")]
    impl NativeDocumentsFixture {
        fn new(contents: &[u8]) -> Self {
            use std::io::Write as _;
            let guard = IOS_DOCUMENTS_TEST_LOCK.lock().unwrap();
            let home = std::env::var_os("HOME").expect("native test container HOME missing");
            let home = std::path::PathBuf::from(home);
            assert!(home.is_absolute());
            let expected_home = std::env::var_os("PHONE_CONTROL_NATIVE_TEST_HOME")
                .map(std::path::PathBuf::from)
                .expect("native acceptance driver must identify its own app container");
            assert!(expected_home.is_absolute());
            assert!(
                home.canonicalize().unwrap() == expected_home.canonicalize().unwrap(),
                "native fixture HOME differs from the driver's own app container"
            );
            let path = home.join("Documents/usage-state.ron");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            assert!(
                !path.with_extension("pending").exists(),
                "native fixture refuses an existing staging file"
            );
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .expect("native fixture requires a fresh owned test container");
            let fixture = Self {
                path,
                cleaned: false,
                _guard: guard,
            };
            file.write_all(contents).unwrap();
            file.sync_all().unwrap();
            drop(file);
            fixture
        }

        fn remove_owned_files(&self) -> std::io::Result<()> {
            // Both names were absent before this fixture's create_new. Its
            // single guarded writer is the production StateStore being tested.
            let pending = self.path.with_extension("pending");
            for path in [&self.path, &pending] {
                match std::fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        }

        fn finish(mut self) {
            self.remove_owned_files().unwrap();
            assert!(!self.path.exists());
            assert!(!self.path.with_extension("pending").exists());
            self.cleaned = true;
        }
    }

    #[cfg(target_os = "ios")]
    impl Drop for NativeDocumentsFixture {
        fn drop(&mut self) {
            if !self.cleaned && self.remove_owned_files().is_err() {
                eprintln!("native Documents test cleanup incomplete");
            }
        }
    }

    #[cfg(target_os = "ios")]
    #[test]
    fn runtime_native_documents_roundtrip_preserves_nondefault_state() {
        let expected = SavedState {
            daily_limit_minutes: 300,
            break_minutes: 45,
            playback_only: true,
            current_day_key: "2026-11-09".into(),
            daily_records: vec![DailyRecord {
                day_key: "2026-11-09".into(),
                label: "11月9日".into(),
                used_ms: 1234,
                playback_only: false,
            }],
            ..SavedState::default()
        };
        let fixture = NativeDocumentsFixture::new(ron::to_string(&expected).unwrap().as_bytes());
        let legacy = LegacyStorage(ron::to_string(&SavedState::default()).unwrap());
        let mut context = eframe::CreationContext::_new_kittest(egui::Context::default());
        context.storage = Some(&legacy);
        let mut model = ControlApp::new(&context, None);
        assert_eq!(model.saved, expected);
        assert!(model.state_store.as_ref().unwrap().is_ready());
        assert!(!model.permission_granted && !model.measurement_available);

        model.saved.daily_limit_minutes = 360;
        let mut frame = eframe::Frame::_new_kittest();
        model.persist_changed_state(&mut frame);
        assert!(model.storage_error.is_none());
        assert_eq!(model.last_persisted.as_ref(), Some(&model.saved));
        let from_disk: SavedState =
            ron::from_str(&std::fs::read_to_string(&fixture.path).unwrap()).unwrap();
        assert_eq!(from_disk, model.saved);
        drop(model);

        let restored = ControlApp::new(&context, None);
        assert_eq!(restored.saved, from_disk);
        assert_eq!(restored.saved.daily_limit_minutes, 360);
        assert!(!restored.permission_granted && !restored.measurement_available);
        let reconstructed_limit = restored.saved.daily_limit_minutes;
        drop(restored);
        fixture.finish();
        println!(
            "runtime_native_documents_roundtrip_verified=true; initial_fixture_limit={}; file_readback_limit={}; reconstructed_limit={reconstructed_limit}; os_permission_verified=false; process_restart_verified=false; owned_fixture_cleanup=true",
            expected.daily_limit_minutes, from_disk.daily_limit_minutes
        );
    }

    #[cfg(target_os = "ios")]
    #[test]
    fn runtime_native_documents_failed_load_blocks_overwrite() {
        let original = b"invalid native Documents fixture";
        let fixture = NativeDocumentsFixture::new(original);
        let context = eframe::CreationContext::_new_kittest(egui::Context::default());
        let mut model = ControlApp::new(&context, None);
        assert!(!model.state_store.as_ref().unwrap().is_ready());
        assert!(model.storage_error.is_some());
        model.saved.daily_limit_minutes = 360;
        let mut frame = eframe::Frame::_new_kittest();
        model.persist_changed_state(&mut frame);
        assert_eq!(std::fs::read(&fixture.path).unwrap(), original);
        assert!(model.last_persisted.is_none());
        struct RecordingStorage(usize);
        impl eframe::Storage for RecordingStorage {
            fn get_string(&self, _: &str) -> Option<String> {
                None
            }
            fn set_string(&mut self, _: &str, _: String) {
                self.0 += 1;
            }
            fn remove_string(&mut self, _: &str) {
                self.0 += 1;
            }
            fn flush(&mut self) {
                self.0 += 1;
            }
        }
        let mut fallback = RecordingStorage(0);
        eframe::App::save(&mut model, &mut fallback);
        assert_eq!(std::fs::read(&fixture.path).unwrap(), original);
        assert_eq!(fallback.0, 0);
        drop(model);
        fixture.finish();
        println!(
            "runtime_native_documents_failed_load_preserved=true; overwrite_blocked=true; owned_fixture_cleanup=true"
        );
    }

    fn legacy_preview_state() -> SavedState {
        let mut preview = app();
        preview.mode = AppMode::Preview;
        preview.saved.target_apps.clear();
        preview.saved.daily_limit_minutes = 300;
        preview.saved.break_minutes = 45;
        preview.saved.playback_only = true;
        preview.saved.cooldown_until_ms = Some(now_epoch_ms() + 30 * 60_000);
        preview.saved.limit_triggered_day = Some("preview".into());
        preview.saved.last_limit_triggered_day = Some("preview".into());
        preview.saved.usage_disclosure_accepted = true;
        preview.saved.service_disclosure_accepted = true;
        preview.load_preview_data();
        preview.saved
    }

    fn assert_preview_session_removed(state: &SavedState) {
        assert!(state.current_day_key.is_empty());
        assert!(
            state
                .target_apps
                .iter()
                .all(|app| !app.package.starts_with("preview."))
        );
        assert!(state.daily_records.is_empty());
        assert_eq!(state.cooldown_until_ms, None);
        assert_eq!(state.limit_triggered_day, None);
        assert_eq!(state.last_limit_triggered_day, None);
        assert!(!state.usage_disclosure_accepted);
        assert!(!state.service_disclosure_accepted);
        assert_eq!(state.daily_limit_minutes, 300);
        assert_eq!(state.break_minutes, 45);
        assert!(state.playback_only);
    }

    struct ReloadFixture(std::path::PathBuf);
    impl ReloadFixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "pfc-runtime-reload-{}-{stamp}-{id}",
                std::process::id()
            ));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> std::path::PathBuf {
            self.0.join("usage-state.ron")
        }
    }
    impl Drop for ReloadFixture {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_file(self.path())
                && error.kind() != std::io::ErrorKind::NotFound
            {
                eprintln!(
                    "test file cleanup deferred: {}: {error}",
                    self.path().display()
                );
            }
            if let Err(error) = std::fs::remove_dir(&self.0) {
                eprintln!(
                    "test directory cleanup deferred: {}: {error}",
                    self.0.display()
                );
            }
        }
    }

    fn recover_store_after_failed_read(
        model: &mut ControlApp,
        state: &SavedState,
        fixture: &ReloadFixture,
    ) {
        let path = fixture.path();
        std::fs::write(&path, b"(corrupt").unwrap();
        let mut store = crate::persistence::StateStore::new(path.clone());
        assert!(store.load_and_migrate::<SavedState>().is_err());
        assert!(!store.is_ready());
        model.state_store = Some(store);
        std::fs::write(&path, ron::to_string(state).unwrap()).unwrap();
        model.last_state_reload = Instant::now() - Duration::from_secs(6);
        let mut frame = eframe::Frame::_new_kittest();
        eframe::App::logic(model, &egui::Context::default(), &mut frame);
        assert!(model.state_store.as_ref().unwrap().is_ready());
    }

    #[test]
    fn runtime_reload_after_read_error_removes_preview_session() {
        let fixture = ReloadFixture::new();
        let mut model = app();
        recover_store_after_failed_read(&mut model, &legacy_preview_state(), &fixture);
        assert_preview_session_removed(&model.saved);
        assert_eq!(model.screen, Screen::Home);
    }

    #[test]
    fn preview_reload_after_read_error_preserves_its_session() {
        let fixture = ReloadFixture::new();
        let mut model = app();
        model.mode = AppMode::Preview;
        let original = legacy_preview_state();
        let persisted: SavedState = ron::from_str(&ron::to_string(&original).unwrap()).unwrap();
        recover_store_after_failed_read(&mut model, &original, &fixture);
        assert_eq!(model.saved, persisted);
        assert_eq!(model.screen, Screen::Break);
    }

    #[test]
    fn runtime_reload_after_read_error_preserves_ordinary_dated_state() {
        let fixture = ReloadFixture::new();
        let mut model = app();
        let mut original = model.saved.clone();
        original.current_day_key = "2026-11-09".into();
        original.daily_limit_minutes = 300;
        original.break_minutes = 45;
        original.daily_records.push(DailyRecord {
            day_key: original.current_day_key.clone(),
            label: "11月9日".into(),
            used_ms: 1234,
            playback_only: false,
        });
        recover_store_after_failed_read(&mut model, &original, &fixture);
        assert_eq!(model.saved, original);
    }

    #[test]
    fn recovered_new_day_state_cannot_reuse_old_measurement_to_begin_a_break() {
        let fixture = ReloadFixture::new();
        let mut model = app();
        model.saved.daily_limit_minutes = 30;
        model.apply_usage_snapshot(snapshot(1, 44));
        model.saved.cooldown_until_ms = None;
        let mut recovered = model.saved.clone();
        recovered.retain_current_day("2026-10-02");
        recover_store_after_failed_read(&mut model, &recovered, &fixture);
        model.accept_calendar_day("2026-10-02");
        model.open_target_app(model.saved.target_apps[0].clone(), egui::Context::default());
        assert_eq!(
            model.saved.cooldown_until_ms, None,
            "recovered state reused yesterday's UI cache"
        );
        assert!(!model.measurement_available);
        assert_eq!(model.today_usage_ms(), 0);
    }

    #[test]
    fn runtime_constructor_preserves_ordinary_legacy_storage() {
        let mut state = app().saved;
        state.current_day_key = "2026-11-09".into();
        state.daily_limit_minutes = 300;
        state.break_minutes = 45;
        state.daily_records.push(DailyRecord {
            day_key: state.current_day_key.clone(),
            label: "11月9日".into(),
            used_ms: 1234,
            playback_only: false,
        });
        let actual = construct_from_legacy(&state);
        assert_eq!(actual.saved, state);
        assert!(!actual.permission_granted);
        assert!(!actual.measurement_available);
    }

    #[test]
    fn runtime_constructor_removes_only_known_preview_session() {
        let mut state = SavedState {
            current_day_key: "preview".into(),
            daily_limit_minutes: 300,
            break_minutes: 45,
            playback_only: true,
            cooldown_until_ms: Some(now_epoch_ms() + 30 * 60_000),
            limit_triggered_day: Some("preview".into()),
            last_limit_triggered_day: Some("preview".into()),
            usage_disclosure_accepted: true,
            service_disclosure_accepted: true,
            target_apps: vec![TrackedApp {
                label: "動画アプリ（プレビュー）".into(),
                package: "preview.video".into(),
            }],
            ..SavedState::default()
        };
        state.daily_records.push(DailyRecord {
            day_key: "preview".into(),
            label: "サンプル".into(),
            used_ms: 102 * 60_000,
            playback_only: false,
        });
        let actual = construct_from_legacy(&state);
        assert_eq!(actual.saved.daily_limit_minutes, 300);
        assert!(actual.saved.target_apps.is_empty());
        assert!(actual.saved.daily_records.is_empty());
        assert!(!actual.permission_granted);
        assert!(!actual.measurement_available);
        assert_preview_session_removed(&actual.saved);
    }

    fn app() -> ControlApp {
        ControlApp {
            mode: AppMode::Runtime,
            saved: SavedState {
                target_apps: vec![TrackedApp {
                    label: "架空動画".into(),
                    package: "test.video".into(),
                }],
                ..SavedState::default()
            },
            last_persisted: None,
            last_persist_at: Instant::now(),
            state_store: None,
            storage_error: None,
            last_state_reload: Instant::now(),
            screen: Screen::Home,
            permission_granted: false,
            measurement_available: false,
            usage_by_package_ms: HashMap::new(),
            today_label: String::new(),
            available_apps: Vec::new(),
            apps_loading: false,
            usage_loading: false,
            app_filter: String::new(),
            notice: None,
            guard_notice: None,
            last_refresh: Instant::now(),
        }
    }

    fn snapshot(day: u32, minutes: u64) -> UsageSnapshot {
        UsageSnapshot {
            permission_granted: true,
            day_key: format!("2026-10-{day:02}"),
            day_label: format!("10月{day}日"),
            now_ms: 1_000_000,
            usage_by_package_ms: HashMap::from([("test.video".into(), minutes * 60_000)]),
            foreground_package: None,
            boot_count: 1,
            measurement_available: true,
        }
    }

    #[test]
    fn runtime_does_not_accept_synthetic_preview_measurements() {
        let mut app = app();
        let original_targets = app.saved.target_apps.clone();
        app.load_preview_data();
        assert!(!app.permission_granted);
        assert!(!app.measurement_available);
        assert!(app.usage_by_package_ms.is_empty());
        assert!(app.saved.daily_records.is_empty());
        assert_eq!(app.saved.target_apps, original_targets);
        app.request_usage_settings(egui::Context::default());
        assert!(!app.permission_granted);
    }

    #[test]
    fn synthetic_measurements_require_an_explicit_preview_mode() {
        let mut app = app();
        app.mode = AppMode::Preview;
        app.load_preview_data();
        assert!(app.permission_granted);
        assert!(app.measurement_available);
        assert_eq!(
            app.usage_by_package_ms.get("preview.video"),
            Some(&(102 * 60_000))
        );
    }

    #[test]
    fn legacy_preview_session_is_not_restored_as_runtime_history() {
        let encoded = ron::to_string(&legacy_preview_state()).unwrap();
        let mut state: SavedState = ron::from_str(&encoded).unwrap();
        state.remove_preview_session();
        assert_eq!(state.daily_limit_minutes, 300);
        assert!(state.target_apps.is_empty());
        assert!(state.daily_records.is_empty());
        assert!(state.current_day_key.is_empty());
        assert!(!state.usage_disclosure_accepted);
        assert!(!state.service_disclosure_accepted);
        assert_preview_session_removed(&state);
    }

    #[test]
    fn runtime_history_is_not_removed_as_preview() {
        let mut state = app().saved;
        state.current_day_key = "2026-11-09".into();
        state.daily_records.push(DailyRecord {
            day_key: state.current_day_key.clone(),
            label: "11月9日".into(),
            used_ms: 1234,
            playback_only: false,
        });
        let before = state.clone();
        state.remove_preview_session();
        assert_eq!(state, before);
    }

    #[test]
    fn stale_service_status_cannot_clear_a_new_break() {
        let mut state = SavedState {
            cooldown_until_ms: Some(2000),
            ..SavedState::default()
        };
        state.accept_service_break(None, None, None, 1000);
        assert_eq!(state.cooldown_until_ms, Some(2000));
        state.accept_service_break(None, None, None, 2000);
        assert_eq!(state.cooldown_until_ms, None);
    }

    #[test]
    fn completed_break_received_from_service_or_save_exits_break_screen() {
        for from_save in [false, true] {
            let mut model = app();
            model.begin_break(now_epoch_ms(), "2026-11-11".into());
            assert_eq!(model.screen, Screen::Break);
            if from_save {
                let mut completed = model.saved.clone();
                completed.cooldown_until_ms = None;
                model.accept_persistence_result(Ok(completed));
            } else {
                model.saved.accept_service_break(
                    None,
                    Some("2026-11-11".into()),
                    Some("2026-11-11".into()),
                    now_epoch_ms(),
                );
            }
            assert_eq!(model.saved.cooldown_until_ms, None);
            eframe::App::logic(
                &mut model,
                &egui::Context::default(),
                &mut eframe::Frame::_new_kittest(),
            );
            assert_eq!(
                model.screen,
                Screen::Home,
                "completed break left its screen active, saved={from_save}"
            );
        }
    }

    #[test]
    fn reconciling_break_screen_keeps_live_break_and_unrelated_settings_screen() {
        let mut model = app();
        model.begin_break(now_epoch_ms(), "2026-11-11".into());
        let deadline = model.saved.cooldown_until_ms;
        eframe::App::logic(
            &mut model,
            &egui::Context::default(),
            &mut eframe::Frame::_new_kittest(),
        );
        assert_eq!(model.screen, Screen::Break);
        assert_eq!(model.saved.cooldown_until_ms, deadline);
        model.saved.cooldown_until_ms = None;
        model.screen = Screen::Settings;
        eframe::App::logic(
            &mut model,
            &egui::Context::default(),
            &mut eframe::Frame::_new_kittest(),
        );
        assert_eq!(model.screen, Screen::Settings);
    }

    #[test]
    fn canonical_save_day_invalidates_yesterdays_ui_measurement_before_open() {
        let fixture = ReloadFixture::new();
        let mut model = app();
        model.saved.daily_limit_minutes = 30;
        model.apply_usage_snapshot(snapshot(1, 44));
        model.saved.cooldown_until_ms = None;
        crate::persistence::write_atomic(&fixture.path(), &model.saved).unwrap();
        let mut store = crate::persistence::StateStore::new(fixture.path());
        store.load::<SavedState>().unwrap();
        let saved =
            crate::guard_storage::save_usage_transaction(&store, &fixture.0, &model.saved, || {
                Ok(("2026-10-02".into(), now_epoch_ms()))
            })
            .unwrap();
        model.accept_persisted_state(saved);
        model.accept_calendar_day("2026-10-02");
        model.open_target_app(model.saved.target_apps[0].clone(), egui::Context::default());
        assert_eq!(
            model.saved.cooldown_until_ms, None,
            "yesterday's cache began a new-day break"
        );
        assert_eq!(model.today_usage_ms(), 0);
        assert!(!model.measurement_available);
        assert!(model.last_refresh.elapsed() >= Duration::from_secs(120));
        std::fs::remove_file(fixture.0.join("guard-config.ron")).unwrap();
    }

    #[test]
    fn reverted_ui_settings_retry_after_a_partial_transaction_failure() {
        let fixture = ReloadFixture::new();
        let mut model = app();
        model.saved.current_day_key = "2026-11-11".into();
        let original = model.saved.clone();
        crate::persistence::write_atomic(&fixture.path(), &original).unwrap();
        let config = fixture.0.join("guard-config.ron");
        crate::persistence::write_atomic(&config, &original).unwrap();
        let mut store = crate::persistence::StateStore::new(fixture.path());
        store.load::<SavedState>().unwrap();
        model.state_store = Some(store);
        model.last_persisted = Some(original.clone());
        model.saved.daily_limit_minutes = 30;
        std::fs::create_dir(config.with_extension("pending")).unwrap();
        let error = crate::guard_storage::save_usage_transaction(
            model.state_store.as_ref().unwrap(),
            &fixture.0,
            &model.saved,
            || Ok(("2026-11-11".into(), 1000)),
        )
        .unwrap_err();
        assert!(!model.accept_persistence_result(Err(error)));
        let partial: SavedState =
            ron::from_str(&std::fs::read_to_string(fixture.path()).unwrap()).unwrap();
        assert_eq!(partial.daily_limit_minutes, 30);
        model.saved = original.clone();
        std::fs::remove_dir(config.with_extension("pending")).unwrap();
        model.persist_changed_state(&mut eframe::Frame::_new_kittest());
        let actual: SavedState =
            ron::from_str(&std::fs::read_to_string(fixture.path()).unwrap()).unwrap();
        std::fs::remove_file(config).unwrap();
        assert_eq!(
            actual.daily_limit_minutes, original.daily_limit_minutes,
            "reverting the UI skipped retry of a partial save"
        );
        assert!(model.storage_error.is_none());
    }

    #[test]
    fn service_completed_break_at_midnight_is_not_restarted_by_ui() {
        let mut app = app();
        app.saved.current_day_key = "2026-10-01".into();
        app.saved.limit_triggered_day = Some("2026-10-02".into());
        app.apply_usage_snapshot(snapshot(2, 130));
        assert_eq!(app.saved.cooldown_until_ms, None);
    }

    #[test]
    fn guard_settings_cannot_skip_the_usage_disclosure() {
        for playback in [false, true] {
            let mut app = app();
            app.request_guard_settings(egui::Context::default(), playback);
            assert_eq!(app.screen, Screen::Disclosure);
            assert!(!app.saved.usage_disclosure_accepted);
            assert!(!app.saved.service_disclosure_accepted);
        }
    }

    #[test]
    fn background_policy_requires_both_disclosures() {
        let mut state = SavedState::default();
        assert!(!state.policy().enabled);
        state.usage_disclosure_accepted = true;
        assert!(!state.policy().enabled);
        state.service_disclosure_accepted = true;
        assert!(state.policy().enabled);
    }

    #[test]
    fn foreground_and_playback_records_for_the_same_day_stay_separate() {
        let mut app = app();
        app.apply_usage_snapshot(snapshot(1, 44));
        app.saved.playback_only = true;
        app.apply_usage_snapshot(snapshot(1, 2));
        assert_eq!(app.saved.daily_records.len(), 2);
        assert_eq!(app.saved.daily_records[0].used_ms, 44 * 60_000);
        assert_eq!(app.saved.daily_records[1].used_ms, 2 * 60_000);
        assert!(app.saved.daily_records[1].playback_only);
    }

    #[test]
    fn saving_legacy_history_uses_current_key_not_latest_date() {
        let mut app = app();
        app.apply_usage_snapshot(snapshot(1, 44));
        let mut future = app.saved.daily_records[0].clone();
        future.day_key = "2099-01-01".into();
        app.saved.daily_records.push(future);
        let deadline = app.saved.cooldown_until_ms;
        let restored: SavedState = ron::from_str(&ron::to_string(&app.saved).unwrap()).unwrap();
        assert_eq!(restored.daily_records.len(), 1);
        assert_eq!(restored.daily_records[0].day_key, app.saved.current_day_key);
        assert_eq!(restored.daily_records[0].used_ms, 44 * 60_000);
        assert_eq!(restored.cooldown_until_ms, deadline);
        assert_eq!(restored.limit_triggered_day, app.saved.limit_triggered_day);
    }
    #[test]
    fn history_keeps_only_the_current_day() {
        let mut app = app();
        for day in 1..=16 {
            app.apply_usage_snapshot(snapshot(day, u64::from(day)));
        }
        assert_eq!(app.saved.daily_records.len(), 1);
        assert_eq!(
            app.saved.daily_records.first().unwrap().day_key,
            "2026-10-16"
        );
        assert_eq!(
            app.saved.daily_records.last().unwrap().day_key,
            "2026-10-16"
        );
        app.apply_usage_snapshot(snapshot(16, 22));
        assert_eq!(app.saved.daily_records.len(), 1);
        assert_eq!(app.saved.daily_records.last().unwrap().used_ms, 22 * 60_000);
    }

    #[test]
    fn known_calendar_without_usage_prunes_old_days_but_keeps_break_and_today_max() {
        let mut model = app();
        model.apply_usage_snapshot(snapshot(1, 44));
        model.saved.cooldown_until_ms = Some(9_000_000);
        model.saved.last_limit_triggered_day = Some("2026-10-01".into());
        model.saved.retain_current_day("2026-10-01");
        assert_eq!(model.saved.daily_records[0].used_ms, 44 * 60_000);
        model.saved.retain_current_day("2026-10-02");
        assert!(model.saved.daily_records.is_empty());
        assert_eq!(model.saved.cooldown_until_ms, Some(9_000_000));
        assert_eq!(
            model.saved.last_limit_triggered_day.as_deref(),
            Some("2026-10-01")
        );
    }

    #[test]
    fn changing_selection_while_permission_is_denied_removes_the_old_aggregate_only() {
        let mut model = app();
        model.apply_usage_snapshot(snapshot(1, 44));
        model.permission_granted = false;
        model.measurement_available = false;
        model.saved.cooldown_until_ms = Some(9_000_000);
        model.saved.last_limit_triggered_day = Some("2026-10-01".into());
        let target = model.saved.target_apps[0].clone();
        model.toggle_app(&target, &egui::Context::default());
        assert!(model.saved.target_apps.is_empty());
        assert!(model.saved.daily_records.is_empty());
        assert_eq!(model.saved.cooldown_until_ms, Some(9_000_000));
        assert_eq!(
            model.saved.last_limit_triggered_day.as_deref(),
            Some("2026-10-01")
        );
    }

    #[test]
    fn calendar_rollover_does_not_credit_yesterdays_in_memory_usage_to_today() {
        let mut model = app();
        model.apply_usage_snapshot(snapshot(1, 44));
        model.saved.daily_limit_minutes = 30;
        model.saved.last_limit_triggered_day = Some("2026-10-01".into());
        model.accept_calendar_day("2026-10-02");
        assert!(!model.measurement_available);
        assert_eq!(model.today_usage_ms(), 0);
        model.open_target_app(model.saved.target_apps[0].clone(), egui::Context::default());
        assert_eq!(model.saved.cooldown_until_ms, None);
        assert_eq!(
            model.saved.last_limit_triggered_day.as_deref(),
            Some("2026-10-01")
        );
    }

    #[test]
    fn unavailable_next_day_snapshot_before_calendar_cannot_retrigger_yesterday() {
        let mut model = app();
        model.apply_usage_snapshot(snapshot(1, 44));
        model.saved.daily_limit_minutes = 30;
        model.saved.last_limit_triggered_day = Some("2026-10-01".into());
        let mut next = snapshot(2, 0);
        next.measurement_available = false;
        model.apply_usage_snapshot(next);
        model.accept_calendar_day("2026-10-02");
        model.open_target_app(model.saved.target_apps[0].clone(), egui::Context::default());
        assert_eq!(model.saved.cooldown_until_ms, None);
        assert!(model.saved.daily_records.is_empty());
        assert_eq!(model.today_usage_ms(), 0);
    }

    #[test]
    fn ui_accepts_service_last_trigger_and_does_not_recreate_a_rolled_back_day() {
        let mut state = SavedState::default();
        state.accept_service_break(None, None, Some("2026-10-02".into()), 1000);
        assert_eq!(
            state.last_limit_triggered_day.as_deref(),
            Some("2026-10-02")
        );
        let mut app = app();
        app.saved.last_limit_triggered_day = Some("2026-10-02".into());
        app.apply_usage_snapshot(snapshot(1, 120));
        assert_eq!(app.saved.cooldown_until_ms, None);
        assert_eq!(app.screen, Screen::Home);
        app.apply_usage_snapshot(snapshot(3, 120));
        assert!(app.saved.cooldown_until_ms.is_some());
    }
    #[test]
    fn stale_service_marker_cannot_move_ui_marker_backward() {
        let mut state = SavedState::default();
        state.accept_service_break(None, None, Some("2026-10-03".into()), 1000);
        state.accept_service_break(None, None, Some("2026-10-02".into()), 2000);
        assert_eq!(
            state.last_limit_triggered_day.as_deref(),
            Some("2026-10-03")
        );
        state.accept_service_break(
            Some(9000),
            Some("2026-10-03".into()),
            Some("2026-10-04".into()),
            3000,
        );
        assert_eq!(state.cooldown_until_ms, Some(9000));
        assert_eq!(state.limit_triggered_day.as_deref(), Some("2026-10-03"));
        assert_eq!(
            state.last_limit_triggered_day.as_deref(),
            Some("2026-10-04")
        );
    }
    #[test]
    fn midnight_does_not_cancel_or_extend_an_active_break() {
        let mut app = app();
        app.apply_usage_snapshot(snapshot(1, 120));
        let deadline = app.saved.cooldown_until_ms;
        assert_eq!(deadline, Some(2_800_000));
        app.apply_usage_snapshot(snapshot(2, 120));
        assert_eq!(app.saved.cooldown_until_ms, deadline);
        assert_eq!(app.screen, Screen::Break);
    }

    #[test]
    fn denied_usage_access_preserves_previous_history() {
        let mut app = app();
        app.apply_usage_snapshot(snapshot(1, 44));
        let saved = app.saved.clone();
        let mut denied = snapshot(1, 0);
        denied.permission_granted = false;
        app.apply_usage_snapshot(denied);
        assert!(!app.permission_granted);
        assert_eq!(app.saved, saved);
        assert_eq!(app.display_usage(), "取得できません");
        assert_eq!(app.today_usage_ms(), 44 * 60_000);
    }

    #[test]
    fn unavailable_playback_does_not_create_a_zero_record_or_clear_observed_time() {
        let mut app = app();
        app.apply_usage_snapshot(snapshot(1, 44));
        let saved = app.saved.clone();
        let mut unavailable = snapshot(1, 0);
        unavailable.measurement_available = false;
        app.apply_usage_snapshot(unavailable);
        assert_eq!(app.saved, saved);
        assert_eq!(app.display_usage(), "取得できません");
        assert_eq!(app.today_usage_ms(), 44 * 60_000);
    }

    #[test]
    fn known_new_day_without_measurement_drops_previous_day_not_break() {
        let mut app = app();
        app.apply_usage_snapshot(snapshot(1, 120));
        let deadline = app.saved.cooldown_until_ms;
        let mut unavailable = snapshot(2, 0);
        unavailable.measurement_available = false;
        app.apply_usage_snapshot(unavailable);
        assert_eq!(app.saved.current_day_key, "2026-10-02");
        assert!(app.saved.daily_records.is_empty());
        assert_eq!(app.saved.cooldown_until_ms, deadline);
        assert_eq!(app.display_usage(), "取得できません");
    }
    #[test]
    fn an_os_history_reset_cannot_erase_a_recorded_day() {
        let mut app = app();
        app.apply_usage_snapshot(snapshot(1, 44));
        app.apply_usage_snapshot(snapshot(1, 0));
        assert_eq!(app.saved.daily_records[0].used_ms, 44 * 60_000);
        app.apply_usage_snapshot(snapshot(1, 47));
        assert_eq!(app.saved.daily_records[0].used_ms, 47 * 60_000);
        app.apply_usage_snapshot(snapshot(2, 1));
        assert_eq!(app.saved.daily_records.len(), 1);
        assert_eq!(app.saved.daily_records[0].used_ms, 60_000);
    }

    #[test]
    fn an_expired_break_is_not_restarted_for_the_same_day() {
        let mut app = app();
        app.apply_usage_snapshot(snapshot(1, 119));
        assert_eq!(app.saved.cooldown_until_ms, None);
        app.apply_usage_snapshot(snapshot(1, 120));
        assert_eq!(app.screen, Screen::Break);
        app.saved.cooldown_until_ms = None;
        app.screen = Screen::Home;
        app.apply_usage_snapshot(snapshot(1, 130));
        assert_eq!(app.saved.cooldown_until_ms, None);
        assert_eq!(app.screen, Screen::Home);
    }

    #[test]
    fn saved_state_round_trip_preserves_an_active_break_and_history() {
        let mut app = app();
        app.apply_usage_snapshot(snapshot(1, 120));
        let encoded = ron::to_string(&app.saved).unwrap();
        let restored: SavedState = ron::from_str(&encoded).unwrap();
        assert_eq!(restored, app.saved);
        assert_eq!(restored.cooldown_until_ms, Some(2_800_000));
        assert_eq!(restored.target_apps[0].package, "test.video");
        assert_eq!(restored.daily_records[0].used_ms, 120 * 60_000);
    }

    #[test]
    fn restored_completed_break_returns_to_home() {
        let mut app = app();
        app.apply_usage_snapshot(snapshot(1, 120));
        assert_eq!(app.screen, Screen::Break);
        let mut restored = app.saved.clone();
        restored.cooldown_until_ms = None;
        app.restore_saved_state(restored);
        assert_eq!(app.screen, Screen::Home);
        assert_eq!(app.saved.cooldown_until_ms, None);
        app.apply_usage_snapshot(snapshot(1, 130));
        assert_eq!(app.screen, Screen::Home);
    }

    #[test]
    fn restored_active_break_opens_break_screen() {
        let mut app = app();
        let restored = SavedState {
            cooldown_until_ms: Some(now_epoch_ms() + 30 * 60_000),
            ..SavedState::default()
        };
        app.restore_saved_state(restored.clone());
        assert_eq!(app.screen, Screen::Break);
        assert_eq!(app.saved, restored);
    }
}
