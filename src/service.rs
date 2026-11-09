//! Android accessibility lifecycle and background guard. Business logic is Rust.
use crate::app::{SavedState, UsageSnapshot};
use crate::enforcement::{GuardState, MediaObservation, Policy};
use crate::platform::{get_system_service, query_selected_usage_env, read_java_string};
use jni::{
    JNIEnv,
    objects::{GlobalRef, JObject, JValue},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const PACKAGE: &str = "com.autodevjapan.phonefrictioncontrol";
static GENERATION: AtomicU64 = AtomicU64::new(0);
static WORKER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static LIVE_STATUS: std::sync::Mutex<Option<GuardStatus>> = std::sync::Mutex::new(None);
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static WAKE_FLAG: std::sync::Mutex<bool> = std::sync::Mutex::new(false);
static WAKE: std::sync::Condvar = std::sync::Condvar::new();
#[cfg(any(feature = "ads-test", feature = "ads-production"))]
pub(crate) fn generation_is_current(generation: u64) -> bool {
    GENERATION.load(Ordering::SeqCst) == generation
}
pub(crate) fn service_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}
pub(crate) fn latest_status() -> Option<GuardStatus> {
    LIVE_STATUS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct GuardStatus {
    pub policy: Policy,
    pub snapshot: UsageSnapshot,
    pub cooldown_until_ms: Option<i64>,
    pub triggered_day: Option<String>,
    pub last_triggered_day: Option<String>,
    pub media_available: bool,
    pub interruption_succeeded: bool,
    pub storage_error: bool,
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_autodevjapan_phonefrictioncontrol_ControlService_onServiceConnected(
    env: JNIEnv,
    service: JObject,
) {
    let result = (|| -> jni::errors::Result<()> {
        let vm = env.get_java_vm()?;
        let service = env.new_global_ref(service)?;
        let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        std::thread::spawn(move || run_guard(vm, service, generation));
        Ok(())
    })();
    if result.is_err() {
        let _ = env.exception_clear();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_autodevjapan_phonefrictioncontrol_ControlService_onAccessibilityEvent(
    _env: JNIEnv,
    _service: JObject,
    _event: JObject,
) {
    // Window events include IME/volume panels, so they only wake the worker.
    // Foreground identity comes exclusively from the replayable UsageEvents.
    *WAKE_FLAG.lock().unwrap_or_else(|p| p.into_inner()) = true;
    WAKE.notify_one();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_autodevjapan_phonefrictioncontrol_ControlService_onInterrupt(
    _env: JNIEnv,
    _service: JObject,
) {
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_autodevjapan_phonefrictioncontrol_ControlService_disconnected(
    _env: JNIEnv,
    _service: JObject,
) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

fn files_dir(env: &mut JNIEnv, context: &JObject) -> jni::errors::Result<PathBuf> {
    let file = env
        .call_method(context, "getFilesDir", "()Ljava/io/File;", &[])?
        .l()?;
    let path = env
        .call_method(&file, "getAbsolutePath", "()Ljava/lang/String;", &[])?
        .l()?;
    Ok(PathBuf::from(read_java_string(env, path)?))
}

fn run_guard(vm: jni::JavaVM, service: GlobalRef, generation: u64) {
    let _worker = WORKER_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    let Ok(dir) = files_dir(&mut env, service.as_obj()) else {
        let _ = env.exception_clear();
        return;
    };
    let mut config_store = crate::persistence::StateStore::new(dir.join("guard-config.ron"));
    let mut last_config: Option<SavedState> = None;
    RUNNING.store(true, Ordering::SeqCst);
    let mut guard_store = crate::persistence::StateStore::new(dir.join("guard-state.ron"));
    let mut state: Option<GuardState> = None;
    let mut last_saved: Option<GuardState> = None;
    let mut last_save = Instant::now() - Duration::from_secs(60);
    let mut last_load = Instant::now() - Duration::from_secs(60);
    let mut tracker = crate::usage::UsageTracker::default();
    let mut last_playing: Option<String> = None;
    let mut last_sample = Instant::now();
    let mut last_interruption = Instant::now() - Duration::from_secs(10);
    let mut last_pause = Instant::now() - Duration::from_secs(10);
    while GENERATION.load(Ordering::SeqCst) == generation {
        if !guard_store.is_ready() && last_load.elapsed() >= Duration::from_secs(5) {
            last_load = Instant::now();
            match guard_store.load_and_migrate::<GuardState>() {
                Ok(restored) => {
                    let mut restored = restored.unwrap_or_default();
                    last_saved = Some(restored.clone());
                    if let Some(memory) = state.take() {
                        restored.merge_unpersisted(memory);
                    }
                    state = Some(restored);
                }
                Err(_) => {
                    // Count new observations in memory without overwriting unreadable history.
                    state.get_or_insert_with(GuardState::default);
                }
            }
        }
        if let Ok(config) = config_store.load::<SavedState>() {
            last_config = config;
        }
        if let Some(state) = &mut state
            && let Some(config) = &last_config
        {
            let policy = config.policy();
            // Explicit selection changes apply even while measurement permission is absent.
            let pruned = state.retain_selected(&policy);
            if pruned && guard_store.is_ready() && guard_store.save(state).is_ok() {
                last_saved = Some(state.clone());
            }
            if policy.enabled {
                let result =
                    env.with_local_frame(64, |env| -> jni::errors::Result<Option<GuardStatus>> {
                        let power = get_system_service(env, service.as_obj(), "power")?;
                        let keyguard = get_system_service(env, service.as_obj(), "keyguard")?;
                        let now = env
                            .call_static_method(
                                "java/lang/System",
                                "currentTimeMillis",
                                "()J",
                                &[],
                            )?
                            .j()?;
                        state.import_break(
                            config.cooldown_until_ms,
                            &config.limit_triggered_day,
                            now,
                        );
                        // A break also stops selected background/PiP sessions. This runs before
                        // the display check; playback measurement still requires a visible screen.
                        if state.cooldown_until_ms.is_some_and(|until| until > now)
                            && last_pause.elapsed() >= Duration::from_secs(2)
                            && GENERATION.load(Ordering::SeqCst) == generation
                        {
                            let _ = pause_selected(env, service.as_obj(), &policy.packages);
                            let _ = env.exception_clear();
                            last_pause = Instant::now();
                        }
                        if !env.call_method(&power, "isInteractive", "()Z", &[])?.z()?
                            || env
                                .call_method(&keyguard, "isKeyguardLocked", "()Z", &[])?
                                .z()?
                        {
                            last_playing = None;
                            last_sample = Instant::now();
                            return Ok(None); // Don't scan usage history while the display is unavailable.
                        }
                        let mut snapshot = query_selected_usage_env(
                            env,
                            service.as_obj(),
                            &policy.packages,
                            true,
                            Some(&mut tracker),
                        )?;
                        let elapsed = last_sample.elapsed().as_millis() as u64;
                        last_sample = Instant::now();
                        if state
                            .cooldown_until_ms
                            .is_some_and(|until| until <= snapshot.now_ms)
                        {
                            state.cooldown_until_ms = None;
                        }
                        let mut media_available = false;
                        let mut should_interrupt = false;
                        if snapshot.permission_granted {
                            let changed_day = state.day != snapshot.day_key;
                            state.sample_usage(
                                &snapshot.day_key,
                                snapshot.boot_count,
                                &snapshot.usage_by_package_ms,
                            );
                            state.retain_selected(&policy);
                            let media = observe_media(
                                env,
                                service.as_obj(),
                                &snapshot.foreground_package,
                                &policy.packages,
                            );
                            if media.is_err() {
                                let _ = env.exception_clear();
                            }
                            let media = media.unwrap_or_default();
                            media_available = media.available(&policy.packages);
                            let playing = media.playing;
                            state.observe_playback(
                                changed_day,
                                playing,
                                &mut last_playing,
                                elapsed,
                            );
                            state.import_break(
                                config.cooldown_until_ms,
                                &config.limit_triggered_day,
                                snapshot.now_ms,
                            );
                            state.enforce(&policy, snapshot.now_ms);
                            snapshot.measurement_available = guard_store.is_ready()
                                && config_store.is_ready()
                                && (!policy.playback_only || media_available);
                            snapshot.usage_by_package_ms = if policy.playback_only {
                                state.playback_ms.clone()
                            } else {
                                state.usage_ms.clone()
                            };
                            should_interrupt = state.cooldown_until_ms.is_some()
                                && snapshot.foreground_package.is_some();
                        } else {
                            last_playing = None;
                        }
                        let critical = last_saved.as_ref().is_none_or(|old| {
                            old.cooldown_until_ms != state.cooldown_until_ms
                                || old.triggered_day != state.triggered_day
                                || old.day != state.day
                        });
                        let mut storage_error = !guard_store.is_ready() || !config_store.is_ready();
                        let mut wrote = false;
                        if last_saved.as_ref() != Some(state)
                            && (critical || last_save.elapsed() >= Duration::from_secs(10))
                        {
                            match guard_store.save(state) {
                                Ok(()) => {
                                    last_saved = Some(state.clone());
                                    last_save = Instant::now();
                                    wrote = true;
                                }
                                Err(_) => {
                                    storage_error = true;
                                }
                            }
                        }
                        let mut interruption_succeeded = false;
                        if state.cooldown_until_ms.is_some()
                            && last_pause.elapsed() >= Duration::from_secs(2)
                            && GENERATION.load(Ordering::SeqCst) == generation
                        {
                            let _ = pause_selected(env, service.as_obj(), &policy.packages);
                            let _ = env.exception_clear();
                            last_pause = Instant::now();
                        }
                        // Keep the in-memory break working even when a persistence attempt fails.
                        if should_interrupt
                            && last_interruption.elapsed() >= Duration::from_secs(2)
                            && GENERATION.load(Ordering::SeqCst) == generation
                        {
                            interruption_succeeded =
                                interrupt(env, service.as_obj()).unwrap_or(false);
                            let _ = env.exception_clear();
                            last_interruption = Instant::now();
                        }
                        let status = GuardStatus {
                            policy,
                            snapshot,
                            cooldown_until_ms: state.cooldown_until_ms,
                            triggered_day: state.triggered_day.clone(),
                            last_triggered_day: state.last_triggered_day.clone(),
                            media_available,
                            interruption_succeeded,
                            storage_error,
                        };
                        if GENERATION.load(Ordering::SeqCst) == generation {
                            *LIVE_STATUS.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(status.clone());
                            // This is a diagnostic/recovery snapshot, not the per-second UI transport.
                            if wrote {
                                let _ = crate::persistence::write_atomic(
                                    &dir.join("guard-status.ron"),
                                    &status,
                                );
                            }
                        }
                        Ok(Some(status))
                    });
                if result.is_err() {
                    let _ = env.exception_clear();
                    last_playing = None;
                    last_sample = Instant::now();
                }
            } else {
                last_playing = None;
                last_sample = Instant::now();
            }
        } else {
            last_playing = None;
            last_sample = Instant::now();
        }
        let flag = WAKE_FLAG.lock().unwrap_or_else(|p| p.into_inner());
        let (mut flag, _) = WAKE
            .wait_timeout_while(flag, Duration::from_secs(1), |flag| !*flag)
            .unwrap_or_else(|p| p.into_inner());
        *flag = false;
    }
    RUNNING.store(false, Ordering::SeqCst);
}
fn active_sessions<'local>(
    env: &mut JNIEnv<'local>,
    context: &JObject,
) -> jni::errors::Result<JObject<'local>> {
    let manager = get_system_service(env, context, "media_session")?;
    let name = JObject::from(env.new_string(format!("{PACKAGE}.MediaListener"))?);
    let component = env.new_object(
        "android/content/ComponentName",
        "(Landroid/content/Context;Ljava/lang/String;)V",
        &[JValue::Object(context), JValue::Object(&name)],
    )?;
    env.call_method(
        &manager,
        "getActiveSessions",
        "(Landroid/content/ComponentName;)Ljava/util/List;",
        &[JValue::Object(&component)],
    )?
    .l()
}

fn observe_media(
    env: &mut JNIEnv,
    context: &JObject,
    foreground: &Option<String>,
    selected: &[String],
) -> jni::errors::Result<MediaObservation> {
    let sessions = active_sessions(env, context)?;
    let mut observation = MediaObservation::default();
    let size = env.call_method(&sessions, "size", "()I", &[])?.i()?;
    for index in 0..size {
        let result = env.with_local_frame(12, |env| -> jni::errors::Result<()> {
            let controller = env
                .call_method(
                    &sessions,
                    "get",
                    "(I)Ljava/lang/Object;",
                    &[JValue::Int(index)],
                )?
                .l()?;
            let package = env
                .call_method(&controller, "getPackageName", "()Ljava/lang/String;", &[])?
                .l()?;
            let package = read_java_string(env, package)?;
            if !selected.contains(&package) {
                return Ok(());
            }
            let state = env
                .call_method(
                    &controller,
                    "getPlaybackState",
                    "()Landroid/media/session/PlaybackState;",
                    &[],
                )?
                .l()?;
            if state.is_null() {
                return Ok(());
            }
            let state = env.call_method(&state, "getState", "()I", &[])?.i()?;
            observation.observe(&package, state, foreground.as_deref());
            Ok(())
        });
        if result.is_err() {
            let _ = env.exception_clear();
        }
    }
    Ok(observation)
}

fn pause_selected(
    env: &mut JNIEnv,
    context: &JObject,
    selected: &[String],
) -> jni::errors::Result<()> {
    let sessions = active_sessions(env, context)?;
    let size = env.call_method(&sessions, "size", "()I", &[])?.i()?;
    for index in 0..size {
        let result = env.with_local_frame(12, |env| -> jni::errors::Result<()> {
            let controller = env
                .call_method(
                    &sessions,
                    "get",
                    "(I)Ljava/lang/Object;",
                    &[JValue::Int(index)],
                )?
                .l()?;
            let package = env
                .call_method(&controller, "getPackageName", "()Ljava/lang/String;", &[])?
                .l()?;
            let package = read_java_string(env, package)?;
            if !selected.contains(&package) {
                return Ok(());
            }
            let state = env
                .call_method(
                    &controller,
                    "getPlaybackState",
                    "()Landroid/media/session/PlaybackState;",
                    &[],
                )?
                .l()?;
            if state.is_null() || env.call_method(&state, "getState", "()I", &[])?.i()? != 3 {
                return Ok(());
            }
            let controls = env
                .call_method(
                    &controller,
                    "getTransportControls",
                    "()Landroid/media/session/MediaController$TransportControls;",
                    &[],
                )?
                .l()?;
            env.call_method(&controls, "pause", "()V", &[])?;
            Ok(())
        });
        if result.is_err() {
            let _ = env.exception_clear();
        }
    }
    Ok(())
}

fn interrupt(env: &mut JNIEnv, context: &JObject) -> jni::errors::Result<bool> {
    #[cfg(any(feature = "ads-test", feature = "ads-production"))]
    if crate::ads::defer_interrupt(env, context, GENERATION.load(Ordering::SeqCst))? {
        return Ok(true);
    }
    interrupt_direct(env, context)
}

pub(crate) fn interrupt_direct(env: &mut JNIEnv, context: &JObject) -> jni::errors::Result<bool> {
    // Global HOME is asynchronous: starting the break immediately afterwards
    // races with HOME and can leave the launcher on top. Switch directly instead.
    let intent = env.new_object("android/content/Intent", "()V", &[])?;
    #[cfg(any(feature = "ads-test", feature = "ads-production"))]
    let activity_class = "com.autodevjapan.phonefrictioncontrol.ControlAdsActivity";
    #[cfg(not(any(feature = "ads-test", feature = "ads-production")))]
    let activity_class = "com.autodevjapan.phonefrictioncontrol.ControlActivity";
    let class = JObject::from(env.new_string(activity_class)?);
    env.call_method(
        &intent,
        "setClassName",
        "(Landroid/content/Context;Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue::Object(context), JValue::Object(&class)],
    )?;
    env.call_method(
        &intent,
        "addFlags",
        "(I)Landroid/content/Intent;",
        &[JValue::Int(0x10000000 | 0x04000000)],
    )?;
    env.call_method(
        context,
        "startActivity",
        "(Landroid/content/Intent;)V",
        &[JValue::Object(&intent)],
    )?;
    Ok(true)
}
