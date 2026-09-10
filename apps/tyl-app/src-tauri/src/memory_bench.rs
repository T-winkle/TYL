//! Opt-in Release benchmark. Memory modes use real translation requests;
//! visual-on/off delegate to deterministic, network-free renderer fixtures.
//! TYL_MEMORY_BENCH=baseline|low|visibility|opaque|gpu-on|gpu-off|visual-on|visual-off.
//! GPU comparison keeps the production LOW policy in both arms.
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::{Listener, Manager};

pub fn mode() -> &'static str {
    static MODE: OnceLock<String> = OnceLock::new();
    MODE.get_or_init(|| match std::env::var("TYL_MEMORY_BENCH").as_deref() {
        Ok(
            value @ ("baseline" | "low" | "visibility" | "opaque" | "gpu-on" | "gpu-off"
            | "visual-on" | "visual-off"),
        ) => value.into(),
        _ => String::new(),
    })
}

pub fn configure(mut context: tauri::Context<tauri::Wry>) -> tauri::Context<tauri::Wry> {
    if mode() == "opaque" {
        for window in &mut context.config_mut().app.windows {
            if window.label == "popup" {
                window.transparent = false;
            }
        }
    }
    context
}

pub fn rendering_config(mut config: crate::settings::Settings) -> crate::settings::Settings {
    if !mode().is_empty() {
        config.log_level = crate::logger::Level::Debug;
    }
    match mode() {
        "gpu-on" | "visual-on" => config.gpu_acceleration = true,
        "gpu-off" | "visual-off" => config.gpu_acceleration = false,
        _ => {}
    }
    config
}

fn stage(name: &str) {
    crate::logger::log_str(&format!("[memory-bench] stage={name} mode={}", mode()));
}

pub fn start(app: tauri::AppHandle, pipeline: Arc<crate::pipeline::Pipeline>) {
    if mode().is_empty() {
        return;
    }
    let listener = app.listen(crate::pipeline::TRANSLATE_EVENT, |event| {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(event.payload()) {
            let phase = value["phase"].as_str().unwrap_or_default();
            if matches!(phase, "done" | "error" | "dict") {
                crate::logger::log_str(&format!(
                    "[memory-bench] result id={} phase={phase} service={}",
                    value["request_id"], value["service"]
                ));
            }
        }
    });
    std::thread::spawn(move || {
        let run = || -> Result<(), String> {
            for _ in 0..100 {
                if app
                    .state::<crate::ReadyFlag>()
                    .0
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if !app
                .state::<crate::ReadyFlag>()
                .0
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err("frontend ready timeout".into());
            }
            stage("startup");
            if matches!(mode(), "visual-on" | "visual-off") {
                return crate::visual_bench::run(&app);
            }
            std::thread::sleep(Duration::from_secs(30));
            let window = app.get_webview_window("popup").ok_or("no popup")?;
            for index in 0..20 {
                stage(&format!("translation-{}", index + 1));
                crate::pipeline::benchmark_present(
                    &app,
                    &pipeline,
                    if index % 2 == 0 {
                        "A quiet morning is a good time to read a book."
                    } else {
                        "elegant"
                    },
                )?;
                std::thread::sleep(Duration::from_secs(3));
                crate::window_ctl::hide(&window);
                std::thread::sleep(Duration::from_secs(1));
            }
            stage("settings-open");
            crate::open_settings_window(&app);
            std::thread::sleep(Duration::from_secs(8));
            if let Some(settings) = app.get_webview_window("settings") {
                settings.close().map_err(|e| e.to_string())?;
            }
            stage("settings-closed");
            std::thread::sleep(Duration::from_secs(5));
            // Start idle timing after a final visible popup and explicit hide.
            crate::pipeline::benchmark_present(
                &app,
                &pipeline,
                "A quiet morning is a good time to read a book.",
            )?;
            std::thread::sleep(Duration::from_secs(3));
            crate::window_ctl::hide(&window);
            stage("hidden");
            std::thread::sleep(Duration::from_secs(30));
            stage("hidden-30s");
            if matches!(mode(), "gpu-on" | "gpu-off") {
                std::thread::sleep(Duration::from_secs(30));
                stage("hidden-60s");
            } else if !matches!(mode(), "opaque" | "visibility") {
                std::thread::sleep(Duration::from_secs(270));
                stage("hidden-5m");
            }
            for index in 0..5 {
                stage(&format!("reopen-{}", index + 1));
                crate::pipeline::benchmark_present(
                    &app,
                    &pipeline,
                    "A quiet morning is a good time to read a book.",
                )?;
                std::thread::sleep(Duration::from_secs(3));
                crate::window_ctl::hide(&window);
                // Test repeated LOW -> NORMAL transitions, not just warm shows.
                std::thread::sleep(Duration::from_secs(17));
            }
            Ok(())
        };
        if let Err(e) = run() {
            crate::logger::log_str(&format!("[memory-bench] ERROR {e}"));
        }
        stage("complete");
        app.unlisten(listener);
        app.exit(0);
    });
}
