//! 任意のバナー広告。試験版と製品版で同じ同意・休憩制御を使う。
//!
//! 同意SDKと広告SDKは、利用者がボタンを押すまで呼ばない。取り消し・ホーム離脱・休憩の後に
//! 届いたコールバックは、呼び出し元オブジェクトと世代番号で捨てる。SDKはロックの外で呼ぶ。
//! 利用時間・ルール・選んだアプリの情報はSDKへ渡さない。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use android_activity::AndroidApp;
use jni::JNIEnv;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};

use crate::ads_state::{BREAK, CLOSED, Event, FAILED, LEFT, Manager, Op, Stage, Step, TOO_SMALL};

struct AdIds {
    app: String,
    banner: String,
}

// GET_META_DATA is required: Context.getApplicationInfo() alone need not include the bundle.
// Read the APK's sole configuration before calling either SDK; never use build-time env IDs.
fn ad_ids(env: &mut JNIEnv, activity: &JObject) -> jni::errors::Result<AdIds> {
    let package = env
        .call_method(activity, "getPackageName", "()Ljava/lang/String;", &[])?
        .l()?;
    let pm = env
        .call_method(
            activity,
            "getPackageManager",
            "()Landroid/content/pm/PackageManager;",
            &[],
        )?
        .l()?;
    let info = env
        .call_method(
            &pm,
            "getApplicationInfo",
            "(Ljava/lang/String;I)Landroid/content/pm/ApplicationInfo;",
            &[JValue::Object(&package), JValue::Int(128)],
        )?
        .l()?;
    let metadata = env
        .get_field(&info, "metaData", "Landroid/os/Bundle;")?
        .l()?;
    if metadata.is_null() {
        return Err(jni::errors::Error::NullPtr("ad metadata missing"));
    }
    let mut read = |key: &str| -> jni::errors::Result<String> {
        let key = JObject::from(env.new_string(key)?);
        let value = env
            .call_method(
                &metadata,
                "getString",
                "(Ljava/lang/String;)Ljava/lang/String;",
                &[JValue::Object(&key)],
            )?
            .l()?;
        if value.is_null() {
            return Err(jni::errors::Error::NullPtr("ad ID missing"));
        }
        let value = JString::from(value);
        let text = env.get_string(&value)?.into();
        Ok(text)
    };
    let ids = AdIds {
        app: read(crate::ad_config::APP_KEY)?,
        banner: read(crate::ad_config::BANNER_KEY)?,
    };
    crate::ad_config::validate(&ids.app, &ids.banner, cfg!(feature = "ads-production"))
        .map_err(jni::errors::Error::NullPtr)?;
    Ok(ids)
}
const GONE: i32 = 8;
const TOP_LEFT: i32 = 0x33;

static MANAGER: Mutex<Manager<GlobalRef>> = Mutex::new(Manager::new());
// 強い参照を持つと Activity が残り続けるため、弱い参照だけを置く。
static OWNER: Mutex<Weak<AndroidApp>> = Mutex::new(Weak::new());

fn lock() -> MutexGuard<'static, Manager<GlobalRef>> {
    MANAGER.lock().unwrap_or_else(|p| p.into_inner())
}

fn owner() -> MutexGuard<'static, Weak<AndroidApp>> {
    OWNER.lock().unwrap_or_else(|p| p.into_inner())
}

pub(crate) struct AdsUi {
    app: Arc<AndroidApp>,
}

impl AdsUi {
    pub(crate) fn new(app: AndroidApp) -> Self {
        let app = Arc::new(app);
        *owner() = Arc::downgrade(&app);
        Self { app }
    }

    pub(crate) fn request(&self) {
        let view = {
            let mut m = lock();
            if m.ad_active() {
                return;
            }
            m.begin(Stage::Update)
        };
        cleanup(&self.app, view);
    }

    pub(crate) fn close(&self) {
        let view = lock().cancel(CLOSED);
        cleanup(&self.app, view);
    }

    pub(crate) fn privacy(&self) {
        let view = lock().begin(Stage::PrivacyUpdate);
        cleanup(&self.app, view);
    }

    /// 毎フレーム呼ぶ。bounds は NativeWindow 基準の物理ピクセル [x, y, 幅, 高さ]。
    pub(crate) fn sync(&self, home_allowed: bool, bounds: Option<[i32; 4]>) {
        let step = lock().step(home_allowed, bounds);
        match step {
            Step::Nothing => {}
            Step::Cleanup(view) => cleanup(&self.app, Some(view)),
            Step::Run(op, epoch) => run(&self.app, op, epoch),
        }
    }

    pub(crate) fn status(&self) -> String {
        let m = lock();
        match m.stage {
            Stage::Idle => m.notice,
            Stage::Update | Stage::Form | Stage::Check => "広告の同意状況を確認しています。",
            Stage::Init | Stage::Ready => {
                if cfg!(feature = "ads-production") {
                    "広告を準備しています。"
                } else {
                    "テスト広告を準備しています。"
                }
            }
            Stage::Loading => {
                if cfg!(feature = "ads-production") {
                    "広告を読み込んでいます。"
                } else {
                    "テスト広告を読み込んでいます。"
                }
            }
            Stage::Shown => {
                if cfg!(feature = "ads-production") {
                    "広告を表示しています。閉じるといつでも消せます。"
                } else {
                    "テスト広告を表示しています。閉じるといつでも消せます。"
                }
            }
            Stage::PrivacyUpdate | Stage::PrivacyForm => "プライバシー設定を開いています。",
        }
        .to_owned()
    }

    pub(crate) fn is_requested(&self) -> bool {
        lock().ad_active()
    }
}

impl Drop for AdsUi {
    fn drop(&mut self) {
        let view = {
            let mut m = lock();
            m.consent = None;
            m.cancel("")
        };
        cleanup(&self.app, view);
        let mut owner = owner();
        if Weak::ptr_eq(&owner, &Arc::downgrade(&self.app)) {
            *owner = Weak::new();
        }
    }
}

/// 休憩へ切り替える前に広告を外す。広告が無ければ false を返し、呼び出し側が直接切り替える。
pub(crate) fn defer_interrupt(
    env: &mut JNIEnv,
    context: &JObject,
    generation: u64,
) -> jni::errors::Result<bool> {
    let Some(app) = owner().upgrade() else {
        return Ok(false);
    };
    let view = {
        let mut m = lock();
        if !m.active() {
            return Ok(false);
        }
        m.cancel(BREAK)
    };
    let Some(view) = view else {
        return Ok(false);
    };
    let prepared = env
        .get_java_vm()
        .and_then(|vm| Ok((vm, env.new_global_ref(context)?)));
    let (vm, context) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            cleanup(&app, Some(view));
            return Err(error);
        }
    };
    app.run_on_java_main_thread(Box::new(move || {
        let Ok(mut env) = vm.attach_current_thread() else {
            return;
        };
        let _ = env.with_local_frame(16, |env| -> jni::errors::Result<()> {
            hide_view(env, view.as_obj());
            let _ = env.exception_clear();
            if crate::service::generation_is_current(generation) {
                let _ = crate::service::interrupt_direct(env, context.as_obj());
                let _ = env.exception_clear();
            }
            Ok(())
        });
    }));
    Ok(true)
}

fn cleanup(app: &Arc<AndroidApp>, view: Option<GlobalRef>) {
    let Some(view) = view else {
        return;
    };
    let main_app = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let _ = crate::platform::with_activity(&main_app, |env, _| {
            hide_view(env, view.as_obj());
            Ok(())
        });
    }));
}

fn hide_view(env: &mut JNIEnv, view: &JObject) {
    let mut destroy = || -> jni::errors::Result<()> {
        let popup = env
            .call_method(view, "getTag", "()Ljava/lang/Object;", &[])?
            .l()?;
        if !popup.is_null() {
            env.call_method(&popup, "dismiss", "()V", &[])?;
            env.call_method(
                view,
                "setTag",
                "(Ljava/lang/Object;)V",
                &[JValue::Object(&JObject::null())],
            )?;
        }
        env.call_method(view, "setVisibility", "(I)V", &[JValue::Int(GONE)])?;
        detach(env, view)?;
        env.call_method(view, "destroy", "()V", &[])?;
        Ok(())
    };
    if destroy().is_err() {
        let _ = env.exception_clear();
        let _ = env.call_method(view, "setVisibility", "(I)V", &[JValue::Int(GONE)]);
        let _ = env.exception_clear();
        let _ = detach(env, view);
        let _ = env.exception_clear();
    }
}

fn detach(env: &mut JNIEnv, view: &JObject) -> jni::errors::Result<()> {
    let parent = env
        .call_method(view, "getParent", "()Landroid/view/ViewParent;", &[])?
        .l()?;
    if !parent.is_null() {
        env.call_method(
            &parent,
            "removeView",
            "(Landroid/view/View;)V",
            &[JValue::Object(view)],
        )?;
    }
    Ok(())
}

fn run(app: &Arc<AndroidApp>, op: Op, epoch: u64) {
    let task_app = app.clone();
    if op == Op::Init {
        // MobileAds.initialize は完了まで戻らないため、UIスレッドでは呼ばない。
        std::thread::spawn(move || {
            {
                let mut m = lock();
                if m.epoch != epoch {
                    m.init_running = false;
                    if m.stage == Stage::Init {
                        m.queued = false;
                    }
                    return;
                }
            }
            let ok = crate::platform::with_activity(&task_app, initialize).is_ok();
            lock().init_done(ok);
        });
        return;
    }
    app.run_on_java_main_thread(Box::new(move || {
        if lock().epoch != epoch {
            return;
        }
        let result = crate::platform::with_activity(&task_app, |env, activity| match op {
            Op::Update => update(env, activity, epoch),
            Op::Form { privacy } => form(env, activity, epoch, privacy),
            Op::Check => check(env, epoch),
            Op::Load(bounds) => load(env, activity, epoch, bounds),
            Op::Layout(bounds) => layout(env, epoch, bounds),
            Op::Init => Ok(()),
        });
        if result.is_err() {
            let view = {
                let mut m = lock();
                if m.epoch == epoch {
                    m.cancel(FAILED)
                } else {
                    None
                }
            };
            if let Some(view) = view {
                let _ = crate::platform::with_activity(&task_app, |env, _| {
                    hide_view(env, view.as_obj());
                    Ok(())
                });
            }
        }
    }));
}

/// SDKのクラスはアプリのクラスローダーから引く（後から接続したスレッドの FindClass では見つからない）。
fn class<'l>(
    env: &mut JNIEnv<'l>,
    activity: &JObject,
    dotted: &str,
) -> jni::errors::Result<JClass<'l>> {
    let loader = env
        .call_method(activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?
        .l()?;
    let name = JObject::from(env.new_string(dotted)?);
    let class = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )?
        .l()?;
    Ok(JClass::from(class))
}

fn callback_object(
    env: &mut JNIEnv,
    activity: &JObject,
    simple: &str,
) -> jni::errors::Result<GlobalRef> {
    let class = class(
        env,
        activity,
        &format!("com.autodevjapan.phonefrictioncontrol.{simple}"),
    )?;
    let object = env.new_object(&class, "()V", &[])?;
    env.new_global_ref(object)
}

fn register(epoch: u64, refs: &[&GlobalRef]) -> bool {
    let mut m = lock();
    if m.epoch != epoch {
        return false;
    }
    m.callbacks
        .extend(refs.iter().map(|r| (epoch, (*r).clone())));
    true
}

fn update(env: &mut JNIEnv, activity: &JObject, epoch: u64) -> jni::errors::Result<()> {
    let ids = ad_ids(env, activity)?;
    let ump = class(
        env,
        activity,
        "com.google.android.ump.UserMessagingPlatform",
    )?;
    let info = env
        .call_static_method(
            &ump,
            "getConsentInformation",
            "(Landroid/content/Context;)Lcom/google/android/ump/ConsentInformation;",
            &[JValue::Object(activity)],
        )?
        .l()?;
    let info = env.new_global_ref(info)?;
    let builder_class = class(
        env,
        activity,
        "com.google.android.ump.ConsentRequestParameters$Builder",
    )?;
    let builder = env.new_object(&builder_class, "()V", &[])?;
    #[cfg(feature = "ads-test")]
    configure_consent_test_geography(env, activity, &builder)?;
    let app_id = JObject::from(env.new_string(&ids.app)?);
    env.call_method(
        &builder,
        "setAdMobAppId",
        "(Ljava/lang/String;)Lcom/google/android/ump/ConsentRequestParameters$Builder;",
        &[JValue::Object(&app_id)],
    )?;
    let params = env
        .call_method(
            &builder,
            "build",
            "()Lcom/google/android/ump/ConsentRequestParameters;",
            &[],
        )?
        .l()?;
    let ok = callback_object(env, activity, "ConsentUpdateSuccess")?;
    let failed = callback_object(env, activity, "ConsentUpdateFailure")?;
    if !register(epoch, &[&ok, &failed]) {
        return Ok(());
    }
    lock().consent = Some(info.clone());
    env.call_method(
        info.as_obj(),
        "requestConsentInfoUpdate",
        "(Landroid/app/Activity;Lcom/google/android/ump/ConsentRequestParameters;Lcom/google/android/ump/ConsentInformation$OnConsentInfoUpdateSuccessListener;Lcom/google/android/ump/ConsentInformation$OnConsentInfoUpdateFailureListener;)V",
        &[
            JValue::Object(activity),
            JValue::Object(&params),
            JValue::Object(ok.as_obj()),
            JValue::Object(failed.as_obj()),
        ],
    )?;
    Ok(())
}

fn form(
    env: &mut JNIEnv,
    activity: &JObject,
    epoch: u64,
    privacy: bool,
) -> jni::errors::Result<()> {
    let dismissed = callback_object(env, activity, "ConsentDismissed")?;
    if !register(epoch, &[&dismissed]) {
        return Ok(());
    }
    let ump = class(
        env,
        activity,
        "com.google.android.ump.UserMessagingPlatform",
    )?;
    env.call_static_method(
        &ump,
        if privacy {
            "showPrivacyOptionsForm"
        } else {
            "loadAndShowConsentFormIfRequired"
        },
        "(Landroid/app/Activity;Lcom/google/android/ump/ConsentForm$OnConsentFormDismissedListener;)V",
        &[JValue::Object(activity), JValue::Object(dismissed.as_obj())],
    )?;
    Ok(())
}

fn check(env: &mut JNIEnv, epoch: u64) -> jni::errors::Result<()> {
    let info = lock()
        .consent
        .clone()
        .ok_or(jni::errors::Error::NullPtr("ConsentInformation"))?;
    let can = env
        .call_method(info.as_obj(), "canRequestAds", "()Z", &[])?
        .z()?;
    lock().checked(epoch, can);
    Ok(())
}

fn initialize(env: &mut JNIEnv, activity: &JObject) -> jni::errors::Result<()> {
    let ids = ad_ids(env, activity)?;
    let context = env
        .call_method(
            activity,
            "getApplicationContext",
            "()Landroid/content/Context;",
            &[],
        )?
        .l()?;
    let state_class = class(
        env,
        activity,
        "com.google.android.libraries.ads.mobile.sdk.common.RequestConfiguration$PublisherPrivacyPersonalizationState",
    )?;
    let disabled = env
        .get_static_field(
            &state_class,
            "DISABLED",
            "Lcom/google/android/libraries/ads/mobile/sdk/common/RequestConfiguration$PublisherPrivacyPersonalizationState;",
        )?
        .l()?;
    let request_builder_class = class(
        env,
        activity,
        "com.google.android.libraries.ads.mobile.sdk.common.RequestConfiguration$Builder",
    )?;
    let request_builder = env.new_object(&request_builder_class, "()V", &[])?;
    env.call_method(
        &request_builder,
        "setPublisherPrivacyPersonalizationState",
        "(Lcom/google/android/libraries/ads/mobile/sdk/common/RequestConfiguration$PublisherPrivacyPersonalizationState;)Lcom/google/android/libraries/ads/mobile/sdk/common/RequestConfiguration$Builder;",
        &[JValue::Object(&disabled)],
    )?;
    let request_config = env
        .call_method(
            &request_builder,
            "build",
            "()Lcom/google/android/libraries/ads/mobile/sdk/common/RequestConfiguration;",
            &[],
        )?
        .l()?;
    let init_builder_class = class(
        env,
        activity,
        "com.google.android.libraries.ads.mobile.sdk.initialization.InitializationConfig$Builder",
    )?;
    let app_id = JObject::from(env.new_string(&ids.app)?);
    let init_builder = env.new_object(
        &init_builder_class,
        "(Ljava/lang/String;)V",
        &[JValue::Object(&app_id)],
    )?;
    const BUILDER: &str =
        "Lcom/google/android/libraries/ads/mobile/sdk/initialization/InitializationConfig$Builder;";
    for name in [
        "disableSdkCrashReporting",
        "disableMediationAdapterInitialization",
    ] {
        env.call_method(&init_builder, name, format!("(){BUILDER}"), &[])?;
    }
    env.call_method(
        &init_builder,
        "setRequestConfiguration",
        format!(
            "(Lcom/google/android/libraries/ads/mobile/sdk/common/RequestConfiguration;){BUILDER}"
        ),
        &[JValue::Object(&request_config)],
    )?;
    let init_config = env
        .call_method(
            &init_builder,
            "build",
            "()Lcom/google/android/libraries/ads/mobile/sdk/initialization/InitializationConfig;",
            &[],
        )?
        .l()?;
    let mobile_ads = class(
        env,
        activity,
        "com.google.android.libraries.ads.mobile.sdk.MobileAds",
    )?;
    env.call_static_method(
        &mobile_ads,
        "initialize",
        "(Landroid/content/Context;Lcom/google/android/libraries/ads/mobile/sdk/initialization/InitializationConfig;)V",
        &[JValue::Object(&context), JValue::Object(&init_config)],
    )?;
    env.call_static_method(
        &mobile_ads,
        "putPublisherFirstPartyIdEnabled",
        "(Z)Z",
        &[JValue::Bool(0)],
    )?;
    Ok(())
}

fn layout_params<'l>(
    env: &mut JNIEnv<'l>,
    [x, y, width, height]: [i32; 4],
) -> jni::errors::Result<JObject<'l>> {
    let params = env.new_object(
        "android/widget/FrameLayout$LayoutParams",
        "(III)V",
        &[
            JValue::Int(width),
            JValue::Int(height),
            JValue::Int(TOP_LEFT),
        ],
    )?;
    env.set_field(&params, "leftMargin", "I", JValue::Int(x))?;
    env.set_field(&params, "topMargin", "I", JValue::Int(y))?;
    Ok(params)
}

fn load(
    env: &mut JNIEnv,
    activity: &JObject,
    epoch: u64,
    bounds: [i32; 4],
) -> jni::errors::Result<()> {
    let ids = ad_ids(env, activity)?;
    let [x, y, width, height] = bounds;
    let resources = env
        .call_method(
            activity,
            "getResources",
            "()Landroid/content/res/Resources;",
            &[],
        )?
        .l()?;
    let metrics = env
        .call_method(
            &resources,
            "getDisplayMetrics",
            "()Landroid/util/DisplayMetrics;",
            &[],
        )?
        .l()?;
    let density = env.get_field(&metrics, "density", "F")?.f()?;
    let width_dp = (width as f32 / density.max(0.1)) as i32;
    let size_class = class(
        env,
        activity,
        "com.google.android.libraries.ads.mobile.sdk.banner.AdSize",
    )?;
    let mut ad_height = 0;
    let mut size = JObject::null();
    if width_dp > 0 {
        size = env
            .call_static_method(
                &size_class,
                "getCurrentOrientationAnchoredAdaptiveBannerAdSize",
                "(Landroid/content/Context;I)Lcom/google/android/libraries/ads/mobile/sdk/banner/AdSize;",
                &[JValue::Object(activity), JValue::Int(width_dp)],
            )?
            .l()?;
        ad_height = env
            .call_method(
                &size,
                "getHeightInPixels",
                "(Landroid/content/Context;)I",
                &[JValue::Object(activity)],
            )?
            .i()?;
    }
    if ad_height <= 0 || ad_height > height {
        let mut m = lock();
        if m.epoch == epoch {
            m.cancel(TOO_SMALL);
        }
        return Ok(());
    }
    let builder_class = class(
        env,
        activity,
        "com.google.android.libraries.ads.mobile.sdk.banner.BannerAdRequest$Builder",
    )?;
    let unit = JObject::from(env.new_string(&ids.banner)?);
    let builder = env.new_object(
        &builder_class,
        "(Ljava/lang/String;Lcom/google/android/libraries/ads/mobile/sdk/banner/AdSize;)V",
        &[JValue::Object(&unit), JValue::Object(&size)],
    )?;
    let request = env
        .call_method(
            &builder,
            "build",
            "()Lcom/google/android/libraries/ads/mobile/sdk/banner/BannerAdRequest;",
            &[],
        )?
        .l()?;
    let view_class = class(
        env,
        activity,
        "com.google.android.libraries.ads.mobile.sdk.banner.AdView",
    )?;
    let view = env.new_object(
        &view_class,
        "(Landroid/content/Context;)V",
        &[JValue::Object(activity)],
    )?;
    let view = env.new_global_ref(view)?;
    let listener = callback_object(env, activity, "AdLoadListener")?;
    {
        let mut m = lock();
        if m.epoch != epoch || m.stage != Stage::Ready {
            drop(m);
            env.call_method(view.as_obj(), "destroy", "()V", &[])?;
            return Ok(());
        }
        m.callbacks.push((epoch, listener.clone()));
        m.view = Some(view.clone());
        m.ad_size = (width, ad_height);
        m.placed = Some(bounds);
        m.set(Stage::Loading);
    }
    let params = layout_params(env, [0, 0, width, ad_height])?;
    env.call_method(
        view.as_obj(),
        "setLayoutParams",
        "(Landroid/view/ViewGroup$LayoutParams;)V",
        &[JValue::Object(&params)],
    )?;
    // NativeActivity owns the window surface, so ordinary content views are hidden
    // by its EGL rendering. A bounded application subwindow has its own surface.
    // It is attached to this Activity only and never uses an overlay permission.
    let popup = env.new_object(
        "android/widget/PopupWindow",
        "(Landroid/view/View;IIZ)V",
        &[
            JValue::Object(view.as_obj()),
            JValue::Int(width),
            JValue::Int(ad_height),
            JValue::Bool(0),
        ],
    )?;
    env.call_method(&popup, "setClippingEnabled", "(Z)V", &[JValue::Bool(1)])?;
    env.call_method(
        view.as_obj(),
        "setTag",
        "(Ljava/lang/Object;)V",
        &[JValue::Object(&popup)],
    )?;
    let window = env
        .call_method(activity, "getWindow", "()Landroid/view/Window;", &[])?
        .l()?;
    let decor = env
        .call_method(&window, "getDecorView", "()Landroid/view/View;", &[])?
        .l()?;
    env.call_method(
        &popup,
        "showAtLocation",
        "(Landroid/view/View;III)V",
        &[
            JValue::Object(&decor),
            JValue::Int(TOP_LEFT),
            JValue::Int(x),
            JValue::Int(y),
        ],
    )?;
    env.call_method(
        view.as_obj(),
        "loadAd",
        "(Lcom/google/android/libraries/ads/mobile/sdk/banner/BannerAdRequest;Lcom/google/android/libraries/ads/mobile/sdk/common/AdLoadCallback;)V",
        &[JValue::Object(&request), JValue::Object(listener.as_obj())],
    )?;
    Ok(())
}

fn layout(env: &mut JNIEnv, epoch: u64, [x, y, _, _]: [i32; 4]) -> jni::errors::Result<()> {
    let (view, (width, height)) = {
        let m = lock();
        match (&m.view, m.epoch == epoch) {
            (Some(view), true) => (view.clone(), m.ad_size),
            _ => return Ok(()),
        }
    };
    let params = layout_params(env, [0, 0, width, height])?;
    env.call_method(
        view.as_obj(),
        "setLayoutParams",
        "(Landroid/view/ViewGroup$LayoutParams;)V",
        &[JValue::Object(&params)],
    )?;
    let popup = env
        .call_method(view.as_obj(), "getTag", "()Ljava/lang/Object;", &[])?
        .l()?;
    if !popup.is_null() {
        env.call_method(
            &popup,
            "update",
            "(IIII)V",
            &[
                JValue::Int(x),
                JValue::Int(y),
                JValue::Int(width),
                JValue::Int(height),
            ],
        )?;
    }
    Ok(())
}

/// コールバックは今保持している呼び出し元オブジェクトと一致するときだけ受け付ける。
fn deliver(env: &mut JNIEnv, this: &JObject, event: Event) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let view = {
            let mut m = lock();
            let found = m
                .callbacks
                .iter()
                .position(|(_, r)| env.is_same_object(r.as_obj(), this).unwrap_or(false));
            let Some(index) = found else {
                return;
            };
            let (epoch, _) = m.callbacks.remove(index);
            m.event(epoch, event)
        };
        // 広告SDKのコールバックはUIスレッドとは限らないため、片付けはUIスレッドへ送る。
        if view.is_some()
            && let Some(app) = owner().upgrade()
        {
            cleanup(&app, view);
        }
    }));
    let _ = env.exception_clear();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_autodevjapan_phonefrictioncontrol_AdLoadListener_onAdLoaded(
    mut env: JNIEnv,
    this: JObject,
    _ad: JObject,
) {
    deliver(&mut env, &this, Event::Loaded);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_autodevjapan_phonefrictioncontrol_AdLoadListener_onAdFailedToLoad(
    mut env: JNIEnv,
    this: JObject,
    _error: JObject,
) {
    deliver(&mut env, &this, Event::LoadFailed);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_autodevjapan_phonefrictioncontrol_ConsentUpdateSuccess_onConsentInfoUpdateSuccess(
    mut env: JNIEnv,
    this: JObject,
) {
    deliver(&mut env, &this, Event::UpdateOk);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_autodevjapan_phonefrictioncontrol_ConsentUpdateFailure_onConsentInfoUpdateFailure(
    mut env: JNIEnv,
    this: JObject,
    _error: JObject,
) {
    deliver(&mut env, &this, Event::UpdateFailed);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_autodevjapan_phonefrictioncontrol_ConsentDismissed_onConsentFormDismissed(
    mut env: JNIEnv,
    this: JObject,
    error: JObject,
) {
    let error = !error.is_null();
    deliver(&mut env, &this, Event::Dismissed { error });
}

/// NativeActivity does not render while paused; invalidate callbacks immediately
/// instead of waiting for another egui frame. This entry is invoked on the UI thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_autodevjapan_phonefrictioncontrol_ControlAdsActivity_onHostPause(
    mut env: JNIEnv,
    this: JObject,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(app) = owner().upgrade() else {
            return;
        };
        let current = crate::platform::with_activity(&app, |current_env, activity| {
            current_env.is_same_object(activity, &this)
        })
        .unwrap_or(false);
        if current {
            let view = lock().cancel(LEFT);
            if let Some(view) = view {
                hide_view(&mut env, view.as_obj());
            }
        }
    }));
    let _ = env.exception_clear();
}

#[cfg(feature = "ads-test")]
fn configure_consent_test_geography(
    env: &mut JNIEnv,
    activity: &JObject,
    parameters: &JObject,
) -> jni::errors::Result<()> {
    let builder_class = class(
        env,
        activity,
        "com.google.android.ump.ConsentDebugSettings$Builder",
    )?;
    let context = env
        .call_method(
            activity,
            "getApplicationContext",
            "()Landroid/content/Context;",
            &[],
        )?
        .l()?;
    let debug = env.new_object(
        &builder_class,
        "(Landroid/content/Context;)V",
        &[JValue::Object(&context)],
    )?;
    let Some(geography) = crate::consent_debug_geography() else {
        return Ok(());
    };
    // UMP DebugGeography.DEBUG_GEOGRAPHY_EEA = 1; emulators are test devices.
    env.call_method(
        &debug,
        "setDebugGeography",
        "(I)Lcom/google/android/ump/ConsentDebugSettings$Builder;",
        &[JValue::Int(geography)],
    )?;
    let settings = env
        .call_method(
            &debug,
            "build",
            "()Lcom/google/android/ump/ConsentDebugSettings;",
            &[],
        )?
        .l()?;
    env.call_method(parameters, "setConsentDebugSettings", "(Lcom/google/android/ump/ConsentDebugSettings;)Lcom/google/android/ump/ConsentRequestParameters$Builder;", &[JValue::Object(&settings)])?;
    Ok(())
}
