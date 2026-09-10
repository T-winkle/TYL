//! Keep the popup warm, but lower WebView2's memory target after 15 idle seconds.
//! LOW does not suspend JavaScript. Never combine this policy with TrySuspend.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tauri::WebviewWindow;

const IDLE_DELAY: Duration = Duration::from_secs(15);
static REVISION: AtomicU64 = AtomicU64::new(0);

fn current(revision: u64) -> bool {
    REVISION.load(Ordering::SeqCst) == revision
}

#[derive(Clone, Copy)]
enum Action {
    Active,
    Hidden,
    Low,
}

// Benchmark variants are impossible to select in a normal build.
fn enabled() -> bool {
    #[cfg(feature = "memory-bench")]
    if matches!(crate::memory_bench::mode(), "baseline" | "opaque") {
        return false;
    }
    cfg!(target_os = "windows")
}

fn low_enabled() -> bool {
    #[cfg(feature = "memory-bench")]
    if crate::memory_bench::mode() == "visibility" {
        return false;
    }
    true
}

/// Startup and every successful hide: sync controller visibility, then debounce LOW.
pub fn hidden(w: &WebviewWindow) {
    if !enabled() {
        return;
    }
    let revision = REVISION.fetch_add(1, Ordering::SeqCst) + 1;
    apply(w, revision, Action::Hidden, None);
    if !low_enabled() {
        return;
    }
    let w = w.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(IDLE_DELAY).await;
        if current(revision) {
            apply(&w, revision, Action::Low, None);
        }
    });
}

/// Queue restoration before show, including the replacement-error recovery path.
pub fn active(w: &WebviewWindow) {
    if enabled() {
        let revision = REVISION.fetch_add(1, Ordering::SeqCst) + 1;
        apply(w, revision, Action::Active, None);
    }
}

/// Capture worker ONLY: wait for UI-thread NORMAL/visibility before emitting events.
/// The UI thread must never block waiting for its own with_webview callback.
pub fn prepare_capture(w: &WebviewWindow) -> Result<(), String> {
    if !enabled() {
        return Ok(());
    }
    let revision = REVISION.fetch_add(1, Ordering::SeqCst) + 1;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    apply(w, revision, Action::Active, Some(tx));
    rx.recv_timeout(Duration::from_secs(1))
        .map_err(|e| format!("WebView activation did not complete: {e}"))?
}

fn apply(
    w: &WebviewWindow,
    revision: u64,
    action: Action,
    done: Option<std::sync::mpsc::SyncSender<Result<(), String>>>,
) {
    #[cfg(target_os = "windows")]
    {
        let popup = w.clone();
        let result = w.with_webview(move |native| {
            let result = (|| -> Result<(), String> {
                if !current(revision) {
                    return Err("superseded WebView transition".into());
                }
                // Recheck on the owning UI thread, not just before queuing the task.
                if matches!(action, Action::Low | Action::Hidden)
                    && popup.is_visible().unwrap_or(true)
                {
                    return Ok(());
                }
                use webview2_com::Microsoft::Web::WebView2::Win32::{
                    ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
                    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
                };
                use windows::core::Interface;
                let started = std::time::Instant::now();
                let controller = native.controller();
                unsafe {
                    let core = controller.CoreWebView2().map_err(|e| e.to_string())?;
                    if !matches!(action, Action::Hidden) && low_enabled() {
                        // Older runtimes still get visibility management, without LOW.
                        match core.cast::<ICoreWebView2_19>() {
                            Ok(core) => {
                                let level = if matches!(action, Action::Low) {
                                    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
                                } else {
                                    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
                                };
                                if let Err(e) = core.SetMemoryUsageTargetLevel(level) {
                                    crate::logger::log_str(&format!(
                                        "[webview-memory] target failed: {e}"
                                    ));
                                }
                            }
                            Err(e) => {
                                static WARNED: std::sync::atomic::AtomicBool =
                                    std::sync::atomic::AtomicBool::new(false);
                                if !WARNED.swap(true, Ordering::Relaxed) {
                                    crate::logger::log_str(&format!(
                                        "[webview-memory] LOW unsupported: {e}"
                                    ));
                                }
                            }
                        }
                    }
                    if !matches!(action, Action::Low) {
                        controller
                            .SetIsVisible(matches!(action, Action::Active))
                            .map_err(|e| e.to_string())?;
                    }
                }
                let state = match action {
                    Action::Active => "NORMAL visible",
                    Action::Hidden => "hidden",
                    Action::Low => "LOW",
                };
                crate::logger::log_str(&format!(
                    "[webview-memory] {state} revision={revision} native_us={}",
                    started.elapsed().as_micros()
                ));
                Ok(())
            })();
            if let Err(ref e) = result {
                crate::logger::log_str(&format!("[webview-memory] {e}"));
            }
            if let Some(done) = done {
                let _ = done.send(result);
            }
        });
        if let Err(e) = result {
            crate::logger::log_str(&format!("[webview-memory] dispatch failed: {e}"));
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (w, revision, action);
        if let Some(done) = done {
            let _ = done.send(Ok(()));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reopening_invalidates_queued_idle_work() {
        use std::sync::atomic::Ordering;
        let hidden = super::REVISION.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(super::current(hidden));
        let reopened = super::REVISION.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(!super::current(hidden));
        assert!(super::current(reopened));
        let hidden_again = super::REVISION.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(!super::current(reopened));
        assert!(super::current(hidden_again));
    }
}
