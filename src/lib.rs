mod app;

#[cfg(target_os = "android")]
mod platform;

pub use app::ControlApp;

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(android_app: android_activity::AndroidApp) {
    use egui_winit::winit::platform::android::EventLoopBuilderExtAndroid;

    let event_loop_app = android_app.clone();
    let app_for_ui = android_app.clone();
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        event_loop_builder: Some(Box::new(move |builder| {
            builder.with_android_app(event_loop_app);
        })),
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
