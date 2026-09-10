//! 平台取词能力在 Tauri 壳里的薄封装：Windows 用真实现，其他平台用桩。
//! （tyl-platform 自带 cfg 门控，这里只做类型适配。）

use tyl_core::capture::CaptureOutcome;
use tyl_core::{CaptureAnchor, MonitorInfo, ScreenLocator};

pub struct CaptureHandle(tyl_platform::backend::WinCapture);

pub fn spawn_capture() -> Option<CaptureHandle> {
    #[cfg(target_os = "windows")]
    {
        // 自动兼容：UIA 优先，必要时使用可还原的剪贴板降级。
        // Auto 仍遵循平台层对终端等危险应用的禁用规则。
        let policy = tyl_core::config::FallbackPolicy::Auto;
        Some(CaptureHandle(
            tyl_platform::backend::WinCapture::with_policy(policy),
        ))
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

impl CaptureHandle {
    pub fn capture_detailed(
        &self,
        anchor: &CaptureAnchor,
        deadline: std::time::Duration,
    ) -> CaptureOutcome {
        #[cfg(target_os = "windows")]
        {
            self.0.capture_detailed(anchor, deadline)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (anchor, deadline);
            CaptureOutcome::Failed(tyl_core::CaptureError::Channel(
                "platform backend not implemented".into(),
            ))
        }
    }
}

pub fn capture_anchor() -> CaptureAnchor {
    tyl_platform::backend::capture_anchor()
}

/// 返回 (显示器列表, 是否可用)。
pub fn monitors() -> (Vec<MonitorInfo>, bool) {
    #[cfg(target_os = "windows")]
    {
        let m = <tyl_platform::backend::WinLocator as Default>::default().monitors();
        let ok = !m.is_empty();
        (m, ok)
    }
    #[cfg(not(target_os = "windows"))]
    {
        (Vec::new(), false)
    }
}

/// 启动预热（COM/UIA 初始化前置；非 Windows 空实现）。
pub fn warmup() {
    #[cfg(target_os = "windows")]
    {
        tyl_platform::backend::warmup();
    }
}
