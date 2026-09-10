//! Process-wide WebView2 policy, frozen before starting any app worker/WebView.
//! Do not mutate it when settings are saved: existing and newly opened windows
//! must share compatible browser environment options until the next restart.
use std::sync::OnceLock;

#[derive(Clone, serde::Serialize)]
pub struct Status {
    pub requested: bool,
    pub software: bool,
    pub external_override: bool,
}

static STARTUP: OnceLock<Status> = OnceLock::new();

fn disables_gpu(arguments: &str) -> bool {
    arguments
        .split_whitespace()
        .any(|arg| arg == "--disable-gpu")
}

fn arguments_for(requested: bool, existing: &str) -> String {
    if requested || disables_gpu(existing) {
        existing.into()
    } else {
        format!("{} --disable-gpu", existing.trim()).trim().into()
    }
}

pub fn initialize(requested: bool) {
    let existing = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
    let external_override = disables_gpu(&existing);
    let arguments = arguments_for(requested, &existing);
    #[cfg(target_os = "windows")]
    if arguments != existing {
        // Local process only; no registry/user/system environment changes.
        std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", &arguments);
    }
    let software = cfg!(target_os = "windows") && disables_gpu(&arguments);
    let _ = STARTUP.set(Status {
        requested,
        software,
        external_override,
    });
    crate::logger::info(&format!(
        "[gpu] startup requested={requested} software={software} external_override={external_override}"
    ));
}

#[tauri::command]
pub fn get_gpu_status() -> Status {
    STARTUP
        .get()
        .expect("GPU policy initialized before WebViews")
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_software_without_overwriting_other_arguments() {
        assert_eq!(arguments_for(false, ""), "--disable-gpu");
        assert_eq!(
            arguments_for(false, "--lang=zh-CN"),
            "--lang=zh-CN --disable-gpu"
        );
        assert_eq!(arguments_for(true, "--lang=zh-CN"), "--lang=zh-CN");
    }

    #[test]
    fn does_not_duplicate_or_silently_remove_external_override() {
        for requested in [false, true] {
            assert_eq!(
                arguments_for(requested, "--disable-gpu --lang=en"),
                "--disable-gpu --lang=en"
            );
        }
        assert!(!disables_gpu("--disable-gpu-compositing"));
        assert!(disables_gpu("--lang=en\t--disable-gpu"));
    }
}
