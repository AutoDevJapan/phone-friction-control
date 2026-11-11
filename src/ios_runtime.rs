use crate::ControlApp;

fn create_app(context: &eframe::CreationContext<'_>) -> ControlApp {
    ControlApp::new(context, None)
}

#[cfg(target_os = "ios")]
pub(crate) fn configure_options(mut options: eframe::NativeOptions) -> eframe::NativeOptions {
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        let original = std::sync::Arc::clone(&setup.device_descriptor);
        setup.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut descriptor = original(adapter);
            // Keep the existing iOS Metal limit repair for both entry points.
            descriptor.required_limits.max_inter_stage_shader_variables = descriptor
                .required_limits
                .max_inter_stage_shader_variables
                .min(adapter.limits().max_inter_stage_shader_variables);
            descriptor
        });
    }
    options
}

/// Launch the existing Runtime model and its separate Documents StateStore.
/// Screen Time authorization, selection, monitoring and shielding are not yet
/// connected by this entry. It does not enable or simulate those capabilities.
#[cfg(target_os = "ios")]
pub fn run() -> eframe::Result {
    let options = configure_options(eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("利用制限")
            .with_inner_size([430.0, 920.0]),
        ..Default::default()
    });
    eframe::run_native(
        "利用制限",
        options,
        Box::new(|context| Ok(Box::new(create_app(context)))),
    )
}

#[cfg(all(test, not(target_os = "ios")))]
mod tests {
    use super::*;
    use crate::app::SavedState;
    use eframe::App as _;

    const KEY: &str = "phone-friction-control-v1";

    struct MemoryStorage(Option<String>);
    impl eframe::Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            (key == KEY).then(|| self.0.clone()).flatten()
        }
        fn set_string(&mut self, key: &str, value: String) {
            assert_eq!(key, KEY);
            self.0 = Some(value);
        }
        fn remove_string(&mut self, key: &str) {
            assert_eq!(key, KEY);
            self.0 = None;
        }
        fn flush(&mut self) {}
    }

    fn construct_and_save(state: SavedState) -> SavedState {
        // Exercise the same factory supplied to eframe::run_native, without
        // creating a window or changing the process HOME or any existing file.
        let original = ron::to_string(&state).unwrap();
        let storage = MemoryStorage(Some(original.clone()));
        let mut context = eframe::CreationContext::_new_kittest(eframe::egui::Context::default());
        context.storage = Some(&storage);
        let mut app = create_app(&context);
        let mut output = MemoryStorage(None);
        app.save(&mut output);
        assert_eq!(storage.0.as_deref(), Some(original.as_str()));
        ron::from_str(output.0.as_deref().expect("Runtime state was not saved")).unwrap()
    }

    #[test]
    fn runtime_entry_preserves_ordinary_saved_settings() {
        let state: SavedState = ron::from_str(
            r#"(daily_limit_minutes:300,break_minutes:45,current_day_key:"2026-11-09",target_apps:[(label:"fixture",package:"test.app")],daily_records:[(day_key:"2026-11-09",label:"11月9日",used_ms:1234,playback_only:false)])"#,
        ).unwrap();
        let restored = construct_and_save(state.clone());
        assert_eq!(
            restored, state,
            "Runtime entry loaded synthetic Preview state"
        );
    }

    #[test]
    fn runtime_entry_removes_legacy_preview_session() {
        let state: SavedState = ron::from_str(
            r#"(daily_limit_minutes:300,break_minutes:45,current_day_key:"preview",target_apps:[(label:"sample",package:"preview.video")],usage_disclosure_accepted:true,service_disclosure_accepted:true)"#,
        ).unwrap();
        let restored = construct_and_save(state);
        assert!(
            restored.current_day_key.is_empty(),
            "Runtime entry retained the Preview day"
        );
        assert!(restored.target_apps.is_empty());
        assert_eq!(restored.daily_limit_minutes, 300);
        assert_eq!(restored.break_minutes, 45);
        assert!(!restored.usage_disclosure_accepted && !restored.service_disclosure_accepted);
    }
}
