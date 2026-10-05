use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;

use android_activity::AndroidApp;
use eframe::egui::Context;
use jni::JavaVM;
use jni::objects::{JObject, JString, JValue};

use crate::app::TrackedApp;
use crate::app::{PlatformEvent, UsageSnapshot};

const OWN_PACKAGE: &str = "com.autodevjapan.phonefrictioncontrol";

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
        let _ = events.send(PlatformEvent::UsageLoaded(result));
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

fn with_activity<T>(
    app: &AndroidApp,
    call: impl FnOnce(&mut jni::JNIEnv<'_>, &JObject<'_>) -> jni::errors::Result<T>,
) -> Result<T, String> {
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }
        .map_err(|error| format!("Java VMを取得できません: {error}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| format!("Androidへ接続できません: {error}"))?;
    let env_raw = env.get_raw();
    let env_table = unsafe { *env_raw };
    let new_local_ref = unsafe { (*env_table).NewLocalRef }
        .ok_or_else(|| "JNIの参照変換を利用できません".to_owned())?;
    let activity_global = app.activity_as_ptr().cast::<jni::sys::_jobject>();
    // SAFETY: android_activity owns this global reference for the lifetime of `app`.
    let activity_local = unsafe { new_local_ref(env_raw, activity_global) };
    if activity_local.is_null() {
        return Err("Android Activityの参照を取得できません".to_owned());
    }
    // SAFETY: NewLocalRef returned a local reference in this attached thread.
    let activity = unsafe { JObject::from_raw(activity_local) };
    call(&mut env, &activity).map_err(|error| format!("Androidの操作に失敗しました: {error}"))
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

        apps.sort_by(|left, right| left.label.to_lowercase().cmp(&right.label.to_lowercase()));
        Ok(apps)
    })
}

fn query_selected_usage(
    app: &AndroidApp,
    selected_packages: &[String],
) -> Result<UsageSnapshot, String> {
    with_activity(app, |env, activity| {
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
        if access_mode == 0 && !selected_packages.is_empty() {
            let usage_manager = get_system_service(env, activity, "usagestats")?;
            let usage_stats = env
                .call_method(
                    &usage_manager,
                    "queryUsageStats",
                    "(IJJ)Ljava/util/List;",
                    &[JValue::Int(0), JValue::Long(start_ms), JValue::Long(now_ms)],
                )?
                .l()?;
            if !usage_stats.is_null() {
                let selected: HashSet<&str> =
                    selected_packages.iter().map(String::as_str).collect();
                let size = env.call_method(&usage_stats, "size", "()I", &[])?.i()?;
                for index in 0..size {
                    let (package, used_ms) =
                        env.with_local_frame(12, |env| -> jni::errors::Result<(String, u64)> {
                            let stats = env
                                .call_method(
                                    &usage_stats,
                                    "get",
                                    "(I)Ljava/lang/Object;",
                                    &[JValue::Int(index)],
                                )?
                                .l()?;
                            let package = env
                                .call_method(&stats, "getPackageName", "()Ljava/lang/String;", &[])?
                                .l()?;
                            let package = read_java_string(env, package)?;
                            let used_ms = env
                                .call_method(&stats, "getTotalTimeInForeground", "()J", &[])?
                                .j()?
                                .max(0) as u64;
                            Ok((package, used_ms))
                        })?;
                    if selected.contains(package.as_str()) {
                        usage_by_package_ms.insert(package, used_ms);
                    }
                }
            }
        }

        Ok(UsageSnapshot {
            permission_granted: access_mode == 0,
            day_key,
            day_label,
            now_ms,
            usage_by_package_ms,
        })
    })
}

fn get_system_service<'local>(
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

fn read_java_string(env: &mut jni::JNIEnv<'_>, value: JObject<'_>) -> jni::errors::Result<String> {
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
