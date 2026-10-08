//! Android accessibility lifecycle and background guard. Business logic is Rust.
use crate::app::{SavedState, UsageSnapshot};
use crate::enforcement::{GuardState, Policy};
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
static TRACKED_PACKAGES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
static EVENT_FOREGROUND: std::sync::Mutex<Option<Option<String>>> = std::sync::Mutex::new(None);
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
        *EVENT_FOREGROUND.lock().unwrap_or_else(|p| p.into_inner()) = None;
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
    mut env: JNIEnv,
    _service: JObject,
    event: JObject,
) {
    // Only the OS package identity; never inspect text, view trees or notification bodies.
    let result = (|| -> jni::errors::Result<()> {
        let name = env
            .call_method(&event, "getPackageName", "()Ljava/lang/CharSequence;", &[])?
            .l()?;
        if name.is_null() {
            return Ok(());
        }
        let string = env
            .call_method(&name, "toString", "()Ljava/lang/String;", &[])?
            .l()?;
        let package = read_java_string(&mut env, string)?;
        let selected = TRACKED_PACKAGES
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&package);
        *EVENT_FOREGROUND.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(selected.then_some(package));
        Ok(())
    })();
    if result.is_err() {
        let _ = env.exception_clear();
    }
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
    let mut guard_store = crate::persistence::StateStore::new(dir.join("guard-state.ron"));
    let mut state: Option<GuardState> = None;
    let mut last_saved: Option<GuardState> = None;
    let mut last_save = Instant::now() - Duration::from_secs(60);
    let mut last_load = Instant::now() - Duration::from_secs(60);
    let mut tracker = crate::usage::UsageTracker::default();
    let mut last_playing: Option<String> = None;
    let mut last_sample = Instant::now();
    let mut last_interruption = Instant::now() - Duration::from_secs(10);
    while GENERATION.load(Ordering::SeqCst) == generation {
        if state.is_none() && last_load.elapsed() >= Duration::from_secs(5) {
            last_load = Instant::now();
            if let Ok(restored) = guard_store.load::<GuardState>() {
                state = Some(restored.unwrap_or_default());
                last_saved = state.clone();
            }
        }
        if let Some(state) = &mut state
            && let Ok(Some(config)) = config_store.load::<SavedState>()
        {
            let policy = config.policy();
            {
                let mut tracked = TRACKED_PACKAGES.lock().unwrap_or_else(|p| p.into_inner());
                if *tracked != policy.packages {
                    *tracked = policy.packages.clone();
                    *EVENT_FOREGROUND.lock().unwrap_or_else(|p| p.into_inner()) = None;
                }
            }
            if policy.enabled {
                let result =
                    env.with_local_frame(64, |env| -> jni::errors::Result<Option<GuardStatus>> {
                        let power = get_system_service(env, service.as_obj(), "power")?;
                        let keyguard = get_system_service(env, service.as_obj(), "keyguard")?;
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
                        if let Some(foreground) = EVENT_FOREGROUND
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .clone()
                        {
                            snapshot.foreground_package = foreground;
                        }
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
                            let media = playing_package(
                                env,
                                service.as_obj(),
                                &snapshot.foreground_package,
                            );
                            media_available = media.is_ok();
                            if !media_available {
                                let _ = env.exception_clear();
                            }
                            let playing = media.unwrap_or(None);
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
                            snapshot.measurement_available =
                                !policy.playback_only || media_available;
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
                        let mut storage_error = false;
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
        std::thread::sleep(Duration::from_secs(1));
    }
}
fn playing_package(
    env: &mut JNIEnv,
    context: &JObject,
    foreground: &Option<String>,
) -> jni::errors::Result<Option<String>> {
    let manager = get_system_service(env, context, "media_session")?;
    let name = JObject::from(env.new_string(format!("{PACKAGE}.MediaListener"))?);
    let component = env.new_object(
        "android/content/ComponentName",
        "(Landroid/content/Context;Ljava/lang/String;)V",
        &[JValue::Object(context), JValue::Object(&name)],
    )?;
    let sessions = env
        .call_method(
            &manager,
            "getActiveSessions",
            "(Landroid/content/ComponentName;)Ljava/util/List;",
            &[JValue::Object(&component)],
        )?
        .l()?;
    let size = env.call_method(&sessions, "size", "()I", &[])?.i()?;
    for index in 0..size {
        let playing = env.with_local_frame(12, |env| -> jni::errors::Result<Option<String>> {
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
            if foreground.as_deref() != Some(package.as_str()) {
                return Ok(None);
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
                return Ok(None);
            }
            let playing = env.call_method(&state, "getState", "()I", &[])?.i()? == 3;
            Ok(playing.then_some(package))
        })?;
        if playing.is_some() {
            return Ok(playing);
        }
    }
    Ok(None)
}

fn interrupt(env: &mut JNIEnv, context: &JObject) -> jni::errors::Result<bool> {
    // Global HOME is asynchronous: starting the break immediately afterwards
    // races with HOME and can leave the launcher on top. Switch directly instead.
    let intent = env.new_object("android/content/Intent", "()V", &[])?;
    let class = JObject::from(env.new_string("android.app.NativeActivity")?);
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
