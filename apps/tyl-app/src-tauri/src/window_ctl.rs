//! 弹窗窗口控制 —— "显示即聚焦"方案（M2 定稿）。
//!
//! 演进记录（教训）：最初用 NOACTIVATE + 动态 focusable + 全局鼠标钩子
//! 的"完全不抢焦点"方案，真机迭代 5 轮仍出边界 case（焦点切换瞬态、
//! tao VISIBLE flag 不同步、SW_HIDE 后 WebView 重建）。结论：与 Windows
//! 焦点模型对抗的复杂度不值。改用 pot/Bob/EasyDict 同款的成熟方案：
//!
//! - 显示 = `SW_SHOW`（激活、获得焦点；原窗口失焦——取词已完成，无损）
//! - 隐藏 = 失焦事件（唯一主路径）+ Esc/✕（前端路径）
//! - 保留 `WS_EX_TOOLWINDOW`：不出现在 Alt+Tab
//! - 钩子/抑止窗/裸 ShowWindow 全部删除
//!
//! 窗口尺寸/位置仍走 Tauri API（物理像素），显示前先定位避免跳变。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tauri::{Emitter, WebviewWindow};

static FOCUS_REVISION: AtomicU64 = AtomicU64::new(0);

/// WebView child-focus transitions and the native move loop can emit transient blur.
/// Only dismiss after the mouse is released and another top-level window owns focus.
pub fn focus_changed(w: &WebviewWindow, focused: bool) {
    // Renderer QA must survive terminal/app focus changes while collecting frames.
    // Excluded from production; this is not a test of focus-loss dismissal.
    #[cfg(feature = "memory-bench")]
    if matches!(crate::memory_bench::mode(), "visual-on" | "visual-off") {
        return;
    }
    let revision = FOCUS_REVISION.fetch_add(1, Ordering::SeqCst) + 1;
    if focused {
        return;
    }
    let w = w.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(140)).await;
            if FOCUS_REVISION.load(Ordering::SeqCst) != revision || !w.is_visible().unwrap_or(false)
            {
                return;
            }
            if pointer_pressed() {
                continue;
            }
            let popup = w.clone();
            let _ = w.run_on_main_thread(move || {
                if FOCUS_REVISION.load(Ordering::SeqCst) == revision
                    && !pointer_pressed()
                    && !owns_foreground(&popup)
                {
                    crate::logger::log_str("[popup] 焦点已移至外部 → 隐藏");
                    hide(&popup);
                }
            });
            return;
        }
    });
}

fn pointer_pressed() -> bool {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON,
        };
        GetAsyncKeyState(i32::from(VK_LBUTTON.0)) < 0
            || GetAsyncKeyState(i32::from(VK_RBUTTON.0)) < 0
    }
    #[cfg(not(target_os = "windows"))]
    false
}

fn owns_foreground(w: &WebviewWindow) -> bool {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetAncestor, GetForegroundWindow, GA_ROOTOWNER,
        };
        if let Ok(hwnd) = w.hwnd() {
            let foreground = GetForegroundWindow();
            return foreground == hwnd || GetAncestor(foreground, GA_ROOTOWNER) == hwnd;
        }
    }
    w.is_focused().unwrap_or(false)
}

/// 启动后调用一次：TOOLWINDOW（不进 Alt+Tab）。
/// 不再打 NOACTIVATE——弹窗需要能接收焦点。
pub fn patch_window_style(w: &WebviewWindow) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
        };
        if let Ok(hwnd) = w.hwnd() {
            unsafe {
                let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                let style = style | WS_EX_TOOLWINDOW.0 as isize;
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
                crate::logger::log_str(&format!(
                    "HWND TOOLWINDOW patch: {:#x} exstyle={style:#x}",
                    hwnd.0 as usize
                ));
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = w;
    }
}

/// 定位并显示（激活聚焦）。pipeline 在取词完成后调用。
/// Tauri 的 show()/set_focus() 走标准激活路径——顺系统而行。
pub fn show_focused(w: &WebviewWindow) {
    FOCUS_REVISION.fetch_add(1, Ordering::SeqCst);
    crate::webview_memory::active(w);
    crate::logger::log_str("show_focused: 显示并聚焦");
    let _ = w.show();
    let _ = w.set_focus();
}

/// 隐藏（失焦/Esc/✕ 路径共用）。
pub fn hide(w: &WebviewWindow) {
    FOCUS_REVISION.fetch_add(1, Ordering::SeqCst);
    crate::logger::log_str("hide");
    if w.hide().is_ok() {
        let _ = w.emit("tyl://popup-hidden", ());
        crate::webview_memory::hidden(w);
    }
}
