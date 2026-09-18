//! Windows backend: anchor snapshot, UIA primary channel, clipboard
//! fallback with snapshot/restore, monitor enumeration.

pub mod anchor;
pub mod clipboard_fallback;
pub mod locator;
pub mod uia_capture;
pub mod uia_selection;

use std::sync::Arc;
use std::time::Duration;

use tyl_core::{capture::CaptureOutcome, CaptureAnchor, CaptureError, CapturedText, TextCapture};

pub use anchor::{capture_anchor, enable_dpi_awareness};
pub use clipboard_fallback::ClipboardFallback;
pub use locator::WinLocator;
pub use uia_capture::UiaCapture;

/// Policy decided from config + anchor, computed before channel dispatch.
#[derive(Debug, Clone, Copy)]
pub enum ChannelPlan {
    /// Try UIA, then clipboard fallback.
    UiaThenClipboard,
    /// UIA only (absolute no-pollution mode or denied app).
    UiaOnly,
    /// Skip UIA (app known-bad for it).
    ClipboardOnly,
}

/// Decides the channel plan for a target process (config-driven).
pub type PlanFn = dyn Fn(Option<&str>) -> ChannelPlan + Send + Sync;

/// The full capture pipeline with channel chaining — this is the object the
/// app/CLI use. Cloning shares the underlying UIA worker thread.
#[derive(Clone)]
pub struct WinCapture {
    uia: UiaCapture,
    fallback: Arc<ClipboardFallback>,
    plan_for: Arc<PlanFn>,
}

impl WinCapture {
    pub fn new(plan_for: impl Fn(Option<&str>) -> ChannelPlan + Send + Sync + 'static) -> Self {
        Self {
            uia: UiaCapture::spawn(),
            fallback: Arc::new(ClipboardFallback::default()),
            plan_for: Arc::new(plan_for),
        }
    }

    /// Default plan derived from a `FallbackPolicy`-carrying config.
    pub fn with_policy(policy: tyl_core::config::FallbackPolicy) -> Self {
        Self::with_policy_and_restore(policy, true)
    }

    /// Like [`with_policy`], with explicit control over clipboard restoration
    /// (CLI `--no-restore`, user setting).
    pub fn with_policy_and_restore(
        policy: tyl_core::config::FallbackPolicy,
        restore: bool,
    ) -> Self {
        let fallback = Arc::new(ClipboardFallback::new(restore));
        Self {
            uia: UiaCapture::spawn(),
            fallback,
            plan_for: Arc::new(move |exe| match policy {
                tyl_core::config::FallbackPolicy::Auto => match exe {
                    // deny-listed processes (conhost) → UIA only
                    Some(e) if is_denied_terminal(e) => ChannelPlan::UiaOnly,
                    _ => ChannelPlan::UiaThenClipboard,
                },
                tyl_core::config::FallbackPolicy::UiaOnly => ChannelPlan::UiaOnly,
                tyl_core::config::FallbackPolicy::ClipboardOnly => ChannelPlan::ClipboardOnly,
            }),
        }
    }

    /// Runs the chain and returns a full outcome (used by CLI `--verbose`
    /// and app telemetry); `capture()` wraps it.
    pub fn capture_detailed(&self, anchor: &CaptureAnchor, deadline: Duration) -> CaptureOutcome {
        let plan = (self.plan_for)(anchor.target_exe.as_deref());
        let uia_budget = deadline.min(Duration::from_millis(300));

        match plan {
            ChannelPlan::ClipboardOnly => {}
            ChannelPlan::UiaOnly | ChannelPlan::UiaThenClipboard => {
                match self.uia.capture(anchor, uia_budget) {
                    Ok(t) => return CaptureOutcome::Primary(t),
                    Err(e) => {
                        clipboard_fallback::trace_log(&format!(
                            "[uia] 取词失败，准备降级剪贴板: {e}"
                        ));
                        if matches!(plan, ChannelPlan::UiaOnly) {
                            // Explain *why* we refuse to fall back — in a
                            // terminal the simulated Ctrl+C is a destructive
                            // interrupt, not a copy.
                            let target = anchor
                                .target_exe
                                .clone()
                                .unwrap_or_else(|| "unknown".into());
                            return CaptureOutcome::Failed(CaptureError::FallbackDisabled(
                                format!(
                                    "{target}: UIA found no selection and the clipboard fallback \
                                 is disabled for terminal apps (simulated Ctrl+C would \
                                 interrupt them)"
                                ),
                            ));
                        }
                        let _ = e; // fall through to clipboard
                    }
                }
            }
        }

        // Clipboard fallback consumes the rest of the budget.
        let fallback_budget = deadline.max(Duration::from_millis(500));
        match self.fallback.capture(anchor, fallback_budget) {
            Ok(mut t) => {
                if t.editable.is_none() {
                    t.editable = self.uia.probe_focused_editable(Duration::from_millis(180));
                }
                CaptureOutcome::Fallback(t)
            }
            Err(e) => CaptureOutcome::Failed(e),
        }
    }
}

impl TextCapture for WinCapture {
    fn capture(
        &self,
        anchor: &CaptureAnchor,
        deadline: Duration,
    ) -> Result<CapturedText, CaptureError> {
        match self.capture_detailed(anchor, deadline) {
            CaptureOutcome::Primary(t) | CaptureOutcome::Fallback(t) => Ok(t),
            CaptureOutcome::Failed(e) => Err(e),
        }
    }
}

/// 启动预热：在后台线程跑一次只读剪贴板快照 + 空 UIA 查询。
/// 把 COM/OLE/UIA 的惰性初始化成本前置到启动期——否则首次真实取词
/// 要承担这些初始化（且未初始化 COM 时快照格式枚举不完整，还原丢
/// 格式 = 污染，真机首发 case）。
pub fn warmup() {
    std::thread::Builder::new()
        .name("tyl-warmup".into())
        .spawn(|| {
            // 1) 剪贴板通道：初始化 COM + 只读 OLE 快照。
            // 不得调用 capture()，否则会向当前前台应用注入 Ctrl+C。
            ClipboardFallback::warmup();
            // 2) UIA 通道：摸一下 STA 线程的 CUIAutomation 实例。
            let uia = UiaCapture::spawn();
            let _ = uia.capture(
                &CaptureAnchor {
                    cursor: None,
                    target_exe: None,
                },
                Duration::from_millis(200),
            );
        })
        .ok();
}

/// Processes where a simulated Ctrl+C would be destructive (SIGINT) or
/// pointless — the built-in deny list mirrors `AppPolicy::with_builtin`.
/// Windows Terminal hosts its own console; conhost/OpenConsole host classic
/// ones. In all of them Ctrl+C is signal-interrupt, not copy.
pub fn is_denied_terminal(exe: &str) -> bool {
    matches!(
        exe.to_ascii_lowercase().as_str(),
        "windowsterminal"
            | "conhost"
            | "openconsole"
            | "microsoft.console"
            | "wt"          // windows terminal alias executable
            | "cmd"         // rarely foreground (conhost is), but cheap to cover
            | "powershell"  // hosted by conhost normally; covers odd setups
            | "pwsh"
    )
}
