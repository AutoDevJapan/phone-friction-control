mod app;
#[cfg(all(feature = "ads-test", feature = "ads-production"))]
compile_error!("ads-test and ads-production are mutually exclusive");

#[cfg(any(
    all(
        target_os = "android",
        any(feature = "ads-test", feature = "ads-production")
    ),
    test
))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod ad_config;

// Dynamic exports survive stripping and let packagers distinguish the native feature.
#[cfg(all(target_os = "android", feature = "ads-test"))]
#[unsafe(no_mangle)]
pub extern "C" fn phone_control_ads_test() -> u32 {
    1
}

#[cfg(all(target_os = "android", feature = "ads-production"))]
#[unsafe(no_mangle)]
pub extern "C" fn phone_control_ads_production() -> u32 {
    1
}
#[cfg(any(target_os = "android", test))]
mod enforcement;
mod persistence;
#[cfg(any(target_os = "android", test))]
mod usage;

#[cfg(target_os = "android")]
mod platform;
#[cfg(target_os = "android")]
mod service;

#[cfg(all(
    target_os = "android",
    any(feature = "ads-test", feature = "ads-production")
))]
mod ads;
#[cfg(any(
    all(
        target_os = "android",
        any(feature = "ads-test", feature = "ads-production")
    ),
    test
))]
mod ads_state;

pub use app::ControlApp;

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(android_app: android_activity::AndroidApp) {
    let Some(data_dir) = android_app.internal_data_path() else {
        eprintln!("端末内の保存先を取得できないため起動を中止しました");
        return;
    };
    let app_for_ui = android_app.clone();
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        android_app: Some(android_app),
        persistence_path: Some(data_dir.join("control-state.ron")),
        ..Default::default()
    };

    if let Err(error) = eframe::run_native(
        "利用制限",
        options,
        Box::new(move |creation_context| {
            Ok(Box::new(ControlApp::new(
                creation_context,
                Some(app_for_ui),
            )))
        }),
    ) {
        eprintln!("アプリを起動できませんでした: {error}");
    }
}

#[cfg(not(target_os = "android"))]
pub fn run_preview() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("利用制限")
            .with_inner_size([430.0, 920.0]),
        ..Default::default()
    };

    eframe::run_native(
        "利用制限",
        options,
        Box::new(|creation_context| Ok(Box::new(ControlApp::new(creation_context, None)))),
    )
}
#[cfg(any(all(target_os = "android", feature = "ads-test"), test))]
pub(crate) const fn consent_debug_geography() -> Option<i32> {
    if cfg!(feature = "ads-test") {
        Some(1)
    } else {
        None
    }
}

#[cfg(test)]
mod consent_debug_tests {
    #[test]
    fn consent_debug_geography_matches_the_selected_build() {
        assert_eq!(
            crate::consent_debug_geography(),
            cfg!(feature = "ads-test").then_some(1)
        );
    }
}
