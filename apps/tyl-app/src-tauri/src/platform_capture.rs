//! Tauri adapters for platform text capture and screen geometry.

use tauri::AppHandle;
use tyl_core::config::FallbackPolicy;
use tyl_core::{MonitorInfo, ScreenLocator};

pub use tyl_platform::backend::capture_anchor;
pub use tyl_platform::Capture as CaptureHandle;

pub fn spawn_capture() -> CaptureHandle {
    CaptureHandle::with_policy(FallbackPolicy::Auto)
}

pub fn monitors(_app: &AppHandle) -> Vec<MonitorInfo> {
    let enumerate = || <tyl_platform::Locator as Default>::default().monitors();

    #[cfg(target_os = "macos")]
    if objc2::MainThreadMarker::new().is_none() {
        let (tx, rx) = std::sync::mpsc::channel();
        if _app
            .run_on_main_thread(move || {
                let _ = tx.send(enumerate());
            })
            .is_err()
        {
            return Vec::new();
        }
        return rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap_or_default();
    }
    enumerate()
}

/// 启动预热（COM/UIA 初始化前置；非 Windows 空实现）。
pub fn warmup() {
    #[cfg(target_os = "windows")]
    {
        tyl_platform::backend::warmup();
    }
}
