use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;

use android_activity::AndroidApp;
use eframe::egui::Context;
use jni::JavaVM;
use jni::objects::{JObject, JString, JValue};

use crate::app::TrackedApp;
use crate::app::{PlatformEvent, UsageSnapshot};

const OWN_PACKAGE: &str = "com.autodevjapan.phonefrictioncontrol";

pub(crate) fn activity_task_id(app: &AndroidApp) -> Result<i32, String> {
    with_activity(app, |env, activity| {
        env.call_method(activity, "getTaskId", "()I", &[])?.i()
    })
}

pub(crate) fn reuse_ui_task_and_retire_duplicate(app: &AndroidApp, owner: i32) {
    let callback_app = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let result = with_activity(&callback_app, |env, activity| {
            let manager = get_system_service(env, activity, "activity")?;
            let tasks = env
                .call_method(&manager, "getAppTasks", "()Ljava/util/List;", &[])?
                .l()?;
            let count = env.call_method(&tasks, "size", "()I", &[])?.i()?;
            let mut found = false;
            for index in 0..count {
                let task = env
                    .call_method(
                        &tasks,
                        "get",
                        "(I)Ljava/lang/Object;",
                        &[JValue::Int(index)],
                    )?
                    .l()?;
                let info = env
                    .call_method(
                        &task,
                        "getTaskInfo",
                        "()Landroid/app/ActivityManager$RecentTaskInfo;",
                        &[],
                    )?
                    .l()?;
                // id becomes -1 while a task is not running. persistentId is
                // the true task identity even while Back has backgrounded it.
                let task_id = env.get_field(&info, "persistentId", "I")?.i()?;
                eprintln!("Native UI task candidate={task_id}, owner={owner}");
                if task_id == owner {
                    env.call_method(&task, "moveToFront", "()V", &[])?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(jni::errors::Error::NullPtr("Owning UI task"));
            }
            let duplicate = env.call_method(activity, "getTaskId", "()I", &[])?.i()?;
            env.call_method(
                activity,
                if duplicate == owner {
                    "finish"
                } else {
                    "finishAndRemoveTask"
                },
                "()V",
                &[],
            )?;
            eprintln!("Reused native UI task={owner}; retiring duplicate task={duplicate}");
            Ok(())
        });
        if let Err(error) = result {
            eprintln!("既存の画面へ戻せません: {error}");
            let _ = with_activity(&callback_app, |env, activity| {
                env.call_method(activity, "finish", "()V", &[])?;
                Ok(())
            });
        }
    }));
    // NativeActivity callbacks wait for this thread. Drain their lifecycle until
    // Destroy so retiring the duplicate cannot block Android's main thread.
    let mut destroyed = false;
    while !destroyed {
        app.poll_events(None, |event| {
            if matches!(
                event,
                android_activity::PollEvent::Main(android_activity::MainEvent::Destroy)
            ) {
                destroyed = true;
            }
        });
    }
}

pub fn request_insets(app: AndroidApp, events: Sender<PlatformEvent>, repaint: Context) {
    let callback_app = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let result = with_activity(&callback_app, |env, activity| {
            let window = env
                .call_method(activity, "getWindow", "()Landroid/view/Window;", &[])?
                .l()?;
            let view = env
                .call_method(&window, "getDecorView", "()Landroid/view/View;", &[])?
                .l()?;
            let insets = env
                .call_method(
                    &view,
                    "getRootWindowInsets",
                    "()Landroid/view/WindowInsets;",
                    &[],
                )?
                .l()?;
            if insets.is_null() {
                return Ok(None);
            }
            let sdk = env
                .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?
                .i()?;
            let mut edges = [0; 4];
            if sdk >= 30 {
                let bars = env
                    .call_static_method("android/view/WindowInsets$Type", "systemBars", "()I", &[])?
                    .i()?;
                let cutout = env
                    .call_static_method(
                        "android/view/WindowInsets$Type",
                        "displayCutout",
                        "()I",
                        &[],
                    )?
                    .i()?;
                let bounds = env
                    .call_method(
                        &insets,
                        "getInsetsIgnoringVisibility",
                        "(I)Landroid/graphics/Insets;",
                        &[JValue::Int(bars | cutout)],
                    )?
                    .l()?;
                for (edge, name) in edges.iter_mut().zip(["left", "top", "right", "bottom"]) {
                    *edge = env.get_field(&bounds, name, "I")?.i()?;
                }
            } else {
                for (edge, name) in edges.iter_mut().zip([
                    "getSystemWindowInsetLeft",
                    "getSystemWindowInsetTop",
                    "getSystemWindowInsetRight",
                    "getSystemWindowInsetBottom",
                ]) {
                    *edge = env.call_method(&insets, name, "()I", &[])?.i()?;
                }
            }
            Ok(Some(edges))
        });
        let _ = events.send(PlatformEvent::InsetsLoaded(result));
        repaint.request_repaint();
    }));
}

pub fn request_apps(app: AndroidApp, events: Sender<PlatformEvent>, repaint: Context) {
    let callback_app = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let result = query_launcher_apps(&callback_app);
        let _ = events.send(PlatformEvent::AppsLoaded(result));
        repaint.request_repaint();
    }));
}

pub fn request_usage(
    app: AndroidApp,
    selected_packages: Vec<String>,
    events: Sender<PlatformEvent>,
    repaint: Context,
) {
    let callback_app = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let result = query_selected_usage(&callback_app, &selected_packages);
        let _ = events.send(PlatformEvent::UsageLoaded {
            selected_packages,
            result,
        });
        repaint.request_repaint();
    }));
}

pub fn open_usage_settings(app: AndroidApp, events: Sender<PlatformEvent>, repaint: Context) {
    open_settings(
        app,
        "android.settings.USAGE_ACCESS_SETTINGS",
        "使用状況へのアクセス設定",
        events,
        repaint,
    );
}

pub fn open_home_settings(app: AndroidApp, events: Sender<PlatformEvent>, repaint: Context) {
    open_settings(
        app,
        "android.settings.HOME_SETTINGS",
        "ホーム画面の選択",
        events,
        repaint,
    );
}

pub fn open_guard_settings(
    app: AndroidApp,
    playback: bool,
    events: Sender<PlatformEvent>,
    repaint: Context,
) {
    open_settings(
        app,
        if playback {
            "android.settings.ACTION_NOTIFICATION_LISTENER_SETTINGS"
        } else {
            "android.settings.ACCESSIBILITY_SETTINGS"
        },
        if playback {
            "通知へのアクセス"
        } else {
            "ユーザー補助"
        },
        events,
        repaint,
    );
}

pub fn return_to_home(app: AndroidApp, events: Sender<PlatformEvent>, repaint: Context) {
    let callback_app = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let result = start_home(&callback_app);
        let _ = events.send(PlatformEvent::ActionDone {
            label: "端末のホーム画面".to_owned(),
            result,
        });
        repaint.request_repaint();
    }));
}

fn open_settings(
    app: AndroidApp,
    action: &'static str,
    label: &'static str,
    events: Sender<PlatformEvent>,
    repaint: Context,
) {
    let callback_app = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let result = start_action(&callback_app, action);
        let _ = events.send(PlatformEvent::ActionDone {
            label: label.to_owned(),
            result,
        });
        repaint.request_repaint();
    }));
}

pub fn launch_package(
    app: AndroidApp,
    package: String,
    label: String,
    events: Sender<PlatformEvent>,
    repaint: Context,
) {
    let callback_app = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let result = start_package(&callback_app, &package);
        let _ = events.send(PlatformEvent::ActionDone { label, result });
        repaint.request_repaint();
    }));
}

pub fn system_dark_mode(app: &AndroidApp) -> Result<bool, String> {
    with_activity(app, |env, activity| {
        let resources = env
            .call_method(
                activity,
                "getResources",
                "()Landroid/content/res/Resources;",
                &[],
            )?
            .l()?;
        let configuration = env
            .call_method(
                &resources,
                "getConfiguration",
                "()Landroid/content/res/Configuration;",
                &[],
            )?
            .l()?;
        let ui_mode = env.get_field(&configuration, "uiMode", "I")?.i()?;
        Ok(ui_mode & 0x30 == 0x20)
    })
}
pub(crate) fn with_activity<T>(
    app: &AndroidApp,
    call: impl FnOnce(&mut jni::JNIEnv<'_>, &JObject<'_>) -> jni::errors::Result<T>,
) -> Result<T, String> {
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }
        .map_err(|error| format!("Java VMを取得できません: {error}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| format!("Androidへ接続できません: {error}"))?;
    let result = env.with_local_frame(64, |env| {
        let env_raw = env.get_raw();
        let env_table = unsafe { *env_raw };
        let new_local_ref = unsafe { (*env_table).NewLocalRef }
            .ok_or(jni::errors::Error::NullPtr("JNI NewLocalRef"))?;
        let activity_global = app.activity_as_ptr().cast::<jni::sys::_jobject>();
        // SAFETY: android_activity owns this global reference for the lifetime of `app`.
        let activity_local = unsafe { new_local_ref(env_raw, activity_global) };
        if activity_local.is_null() {
            return Err(jni::errors::Error::NullPtr("Android Activity"));
        }
        // SAFETY: NewLocalRef returned a reference inside this local frame.
        let activity = unsafe { JObject::from_raw(activity_local) };
        call(env, &activity)
    });
    if result.is_err() {
        // A caught Java exception must not leak into later callbacks on the UI thread.
        let _ = env.exception_clear();
    }
    result.map_err(|error| format!("Androidの操作に失敗しました: {error}"))
}

fn query_launcher_apps(app: &AndroidApp) -> Result<Vec<TrackedApp>, String> {
    with_activity(app, |env, activity| {
        let package_manager = env
            .call_method(
                activity,
                "getPackageManager",
                "()Landroid/content/pm/PackageManager;",
                &[],
            )?
            .l()?;
        let intent = env.new_object("android/content/Intent", "()V", &[])?;
        let action = env.new_string("android.intent.action.MAIN")?;
        let action_object = JObject::from(action);
        env.call_method(
            &intent,
            "setAction",
            "(Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&action_object)],
        )?;
        let category = env.new_string("android.intent.category.LAUNCHER")?;
        let category_object = JObject::from(category);
        env.call_method(
            &intent,
            "addCategory",
            "(Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&category_object)],
        )?;
        let activities = env
            .call_method(
                &package_manager,
                "queryIntentActivities",
                "(Landroid/content/Intent;I)Ljava/util/List;",
                &[JValue::Object(&intent), JValue::Int(0)],
            )?
            .l()?;
        let size = env.call_method(&activities, "size", "()I", &[])?.i()?;
        let mut seen_packages = HashSet::new();
        let mut apps = Vec::new();

        for index in 0..size {
            let app =
                env.with_local_frame(16, |env| -> jni::errors::Result<Option<TrackedApp>> {
                    let resolve_info = env
                        .call_method(
                            &activities,
                            "get",
                            "(I)Ljava/lang/Object;",
                            &[JValue::Int(index)],
                        )?
                        .l()?;
                    let activity_info = env
                        .get_field(
                            &resolve_info,
                            "activityInfo",
                            "Landroid/content/pm/ActivityInfo;",
                        )?
                        .l()?;
                    let package_object = env
                        .get_field(&activity_info, "packageName", "Ljava/lang/String;")?
                        .l()?;
                    let package = read_java_string(env, package_object)?;
                    if package == OWN_PACKAGE || !seen_packages.insert(package.clone()) {
                        return Ok(None);
                    }

                    let label_object = env
                        .call_method(
                            &resolve_info,
                            "loadLabel",
                            "(Landroid/content/pm/PackageManager;)Ljava/lang/CharSequence;",
                            &[JValue::Object(&package_manager)],
                        )?
                        .l()?;
                    let java_label = env
                        .call_method(&label_object, "toString", "()Ljava/lang/String;", &[])?
                        .l()?;
                    let label = read_java_string(env, java_label)?;
                    Ok(Some(TrackedApp { label, package }))
                })?;
            if let Some(app) = app {
                apps.push(app);
            }
        }

        apps.sort_by_key(|app| app.label.to_lowercase());
        Ok(apps)
    })
}

fn query_selected_usage(
    app: &AndroidApp,
    selected_packages: &[String],
) -> Result<UsageSnapshot, String> {
    with_activity(app, |env, activity| {
        query_selected_usage_env(env, activity, selected_packages, false, None)
    })
}

pub(crate) fn current_day(app: &AndroidApp) -> Result<String, String> {
    with_activity(app, |env, _| current_day_env(env))
}

pub(crate) fn storage_calendar(
    app: &AndroidApp,
) -> std::io::Result<crate::guard_storage::Calendar> {
    with_activity(app, |env, _| storage_calendar_env_raw(env)).map_err(std::io::Error::other)
}

pub(crate) fn storage_calendar_env(
    env: &mut jni::JNIEnv<'_>,
) -> std::io::Result<crate::guard_storage::Calendar> {
    let result = storage_calendar_env_raw(env);
    if result.is_err() {
        let _ = env.exception_clear();
    }
    result.map_err(std::io::Error::other)
}

fn storage_calendar_env_raw(
    env: &mut jni::JNIEnv<'_>,
) -> jni::errors::Result<crate::guard_storage::Calendar> {
    env.with_local_frame(8, |env| {
        let calendar = env
            .call_static_method(
                "java/util/Calendar",
                "getInstance",
                "()Ljava/util/Calendar;",
                &[],
            )?
            .l()?;
        let now = env
            .call_method(&calendar, "getTimeInMillis", "()J", &[])?
            .j()?;
        let year = env
            .call_method(&calendar, "get", "(I)I", &[JValue::Int(1)])?
            .i()?;
        let month = env
            .call_method(&calendar, "get", "(I)I", &[JValue::Int(2)])?
            .i()?
            + 1;
        let day = env
            .call_method(&calendar, "get", "(I)I", &[JValue::Int(5)])?
            .i()?;
        Ok((format!("{year:04}-{month:02}-{day:02}"), now))
    })
}

pub(crate) fn current_day_env(env: &mut jni::JNIEnv<'_>) -> jni::errors::Result<String> {
    env.with_local_frame(8, |env| {
        let calendar = env
            .call_static_method(
                "java/util/Calendar",
                "getInstance",
                "()Ljava/util/Calendar;",
                &[],
            )?
            .l()?;
        let year = env
            .call_method(&calendar, "get", "(I)I", &[JValue::Int(1)])?
            .i()?;
        let month = env
            .call_method(&calendar, "get", "(I)I", &[JValue::Int(2)])?
            .i()?
            + 1;
        let day = env
            .call_method(&calendar, "get", "(I)I", &[JValue::Int(5)])?
            .i()?;
        Ok(format!("{year:04}-{month:02}-{day:02}"))
    })
}

pub(crate) fn query_selected_usage_env(
    env: &mut jni::JNIEnv<'_>,
    activity: &JObject<'_>,
    selected_packages: &[String],
    current_boot_only: bool,
    tracker: Option<&mut crate::usage::UsageTracker>,
) -> jni::errors::Result<UsageSnapshot> {
    let now_ms = env
        .call_static_method("java/lang/System", "currentTimeMillis", "()J", &[])?
        .j()?;
    let calendar = env
        .call_static_method(
            "java/util/Calendar",
            "getInstance",
            "()Ljava/util/Calendar;",
            &[],
        )?
        .l()?;
    for field in [11, 12, 13, 14] {
        env.call_method(
            &calendar,
            "set",
            "(II)V",
            &[JValue::Int(field), JValue::Int(0)],
        )?;
    }
    let start_ms = env
        .call_method(&calendar, "getTimeInMillis", "()J", &[])?
        .j()?;
    let year = env
        .call_method(&calendar, "get", "(I)I", &[JValue::Int(1)])?
        .i()?;
    let month = env
        .call_method(&calendar, "get", "(I)I", &[JValue::Int(2)])?
        .i()?
        + 1;
    let day = env
        .call_method(&calendar, "get", "(I)I", &[JValue::Int(5)])?
        .i()?;
    let day_key = format!("{year:04}-{month:02}-{day:02}");
    let day_label = format!("{month}月{day}日");
    let resolver = env
        .call_method(
            activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )?
        .l()?;
    let boot_key = JObject::from(env.new_string("boot_count")?);
    let boot_count = env
        .call_static_method(
            "android/provider/Settings$Global",
            "getInt",
            "(Landroid/content/ContentResolver;Ljava/lang/String;I)I",
            &[
                JValue::Object(&resolver),
                JValue::Object(&boot_key),
                JValue::Int(-1),
            ],
        )?
        .i()?;
    let uptime = env
        .call_static_method("android/os/SystemClock", "elapsedRealtime", "()J", &[])?
        .j()?;
    let boot_start = now_ms.saturating_sub(uptime);
    let observation_start = if current_boot_only {
        start_ms.max(boot_start)
    } else {
        start_ms
    };

    let app_ops = get_system_service(env, activity, "appops")?;
    let package_name = env
        .call_method(activity, "getPackageName", "()Ljava/lang/String;", &[])?
        .l()?;
    let operation = env.new_string("android:get_usage_stats")?;
    let operation_object = JObject::from(operation);
    let uid = env
        .call_static_method("android/os/Process", "myUid", "()I", &[])?
        .i()?;
    let access_mode = env
        .call_method(
            &app_ops,
            "checkOpNoThrow",
            "(Ljava/lang/String;ILjava/lang/String;)I",
            &[
                JValue::Object(&operation_object),
                JValue::Int(uid),
                JValue::Object(&package_name),
            ],
        )?
        .i()?;
    let mut usage_by_package_ms = HashMap::new();
    let mut foreground_package = None;
    if access_mode == 0 && !selected_packages.is_empty() {
        let mut working = tracker.as_deref().cloned().unwrap_or_default();
        let earliest = if current_boot_only {
            observation_start.saturating_sub(86_400_000).max(boot_start)
        } else {
            observation_start.saturating_sub(86_400_000)
        };
        let query_begin = working.prepare(
            &day_key,
            boot_count,
            selected_packages,
            observation_start,
            now_ms,
            earliest,
        );
        let usage_manager = get_system_service(env, activity, "usagestats")?;
        let events = env
            .call_method(
                &usage_manager,
                "queryEvents",
                "(JJ)Landroid/app/usage/UsageEvents;",
                &[JValue::Long(query_begin), JValue::Long(now_ms)],
            )?
            .l()?;
        // A null result on a locked Android user is not a measured zero.
        if events.is_null() {
            return Err(jni::errors::Error::NullPtr("UsageEvents unavailable"));
        }
        let event = env.new_object("android/app/usage/UsageEvents$Event", "()V", &[])?;
        while env.call_method(&events, "hasNextEvent", "()Z", &[])?.z()? {
            if !env
                .call_method(
                    &events,
                    "getNextEvent",
                    "(Landroid/app/usage/UsageEvents$Event;)Z",
                    &[JValue::Object(&event)],
                )?
                .z()?
            {
                break;
            }
            let kind = env.call_method(&event, "getEventType", "()I", &[])?.i()?;
            let kind = match kind {
                1 => crate::usage::EventKind::Resumed,
                2 | 23 => crate::usage::EventKind::Paused,
                16 | 17 => crate::usage::EventKind::ScreenOff,
                26 | 27 => crate::usage::EventKind::Shutdown,
                _ => continue,
            };
            let at_ms = env.call_method(&event, "getTimeStamp", "()J", &[])?.j()?;
            if at_ms >= now_ms {
                continue;
            }
            env.with_local_frame(8, |env| -> jni::errors::Result<()> {
                let package = env
                    .call_method(&event, "getPackageName", "()Ljava/lang/String;", &[])?
                    .l()?;
                let class = env
                    .call_method(&event, "getClassName", "()Ljava/lang/String;", &[])?
                    .l()?;
                let package = if package.is_null() {
                    String::new()
                } else {
                    read_java_string(env, package)?
                };
                let class = if class.is_null() {
                    String::new()
                } else {
                    read_java_string(env, class)?
                };
                working.event(at_ms, &package, &class, kind);
                Ok(())
            })?;
        }
        (usage_by_package_ms, foreground_package) = working.result(now_ms);
        if let Some(tracker) = tracker {
            *tracker = working;
        }
    }

    Ok(UsageSnapshot {
        permission_granted: access_mode == 0,
        day_key,
        day_label,
        now_ms,
        usage_by_package_ms,
        foreground_package,
        boot_count,
        measurement_available: access_mode == 0,
    })
}

pub(crate) fn get_system_service<'local>(
    env: &mut jni::JNIEnv<'local>,
    activity: &JObject<'_>,
    name: &str,
) -> jni::errors::Result<JObject<'local>> {
    let name = env.new_string(name)?;
    let name_object = JObject::from(name);
    env.call_method(
        activity,
        "getSystemService",
        "(Ljava/lang/String;)Ljava/lang/Object;",
        &[JValue::Object(&name_object)],
    )?
    .l()
}

pub(crate) fn read_java_string(
    env: &mut jni::JNIEnv<'_>,
    value: JObject<'_>,
) -> jni::errors::Result<String> {
    let value = JString::from(value);
    Ok(env.get_string(&value)?.into())
}

fn start_package(app: &AndroidApp, package: &str) -> Result<(), String> {
    with_activity(app, |env, activity| {
        let package_manager = env
            .call_method(
                activity,
                "getPackageManager",
                "()Landroid/content/pm/PackageManager;",
                &[],
            )?
            .l()?;
        let package_string = env.new_string(package)?;
        let package_object = JObject::from(package_string);
        let intent = env
            .call_method(
                &package_manager,
                "getLaunchIntentForPackage",
                "(Ljava/lang/String;)Landroid/content/Intent;",
                &[JValue::Object(&package_object)],
            )?
            .l()?;
        if intent.is_null() {
            return Err(jni::errors::Error::NullPtr("Launch intent was not found"));
        }
        env.call_method(
            activity,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[JValue::Object(&intent)],
        )?;
        Ok(())
    })
}

fn start_action(app: &AndroidApp, action: &str) -> Result<(), String> {
    with_activity(app, |env, activity| {
        let action_name = env.new_string(action)?;
        let action_object = JObject::from(action_name);
        let intent = env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&action_object)],
        )?;
        env.call_method(
            activity,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[JValue::Object(&intent)],
        )?;
        Ok(())
    })
}

fn start_home(app: &AndroidApp) -> Result<(), String> {
    with_activity(app, |env, activity| {
        let intent = env.new_object("android/content/Intent", "()V", &[])?;
        let action = env.new_string("android.intent.action.MAIN")?;
        let action_object = JObject::from(action);
        env.call_method(
            &intent,
            "setAction",
            "(Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&action_object)],
        )?;
        for category in [
            "android.intent.category.HOME",
            "android.intent.category.DEFAULT",
        ] {
            let category = env.new_string(category)?;
            let category_object = JObject::from(category);
            env.call_method(
                &intent,
                "addCategory",
                "(Ljava/lang/String;)Landroid/content/Intent;",
                &[JValue::Object(&category_object)],
            )?;
        }
        env.call_method(
            activity,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[JValue::Object(&intent)],
        )?;
        Ok(())
    })
}
