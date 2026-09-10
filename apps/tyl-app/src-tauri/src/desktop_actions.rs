//! Explicit user actions: copy a result, or paste into the verified original selection.

use std::sync::Mutex;

#[derive(Clone, Copy, Default)]
pub struct TargetWindow {
    handle: usize,
    process_id: u32,
    focus_handle: usize,
}

#[derive(Clone)]
struct Selection {
    request_id: u64,
    target: TargetWindow,
    original: String,
    editable: bool,
    selection_id: Option<Vec<i32>>,
}

static SELECTION: Mutex<Option<Selection>> = Mutex::new(None);

/// Missing accessibility metadata is common in custom editors. An explicit
/// user click may attempt a verified paste; known read-only ranges cannot.
pub fn can_offer_replacement(editable: Option<bool>, target_exe: Option<&str>) -> bool {
    #[cfg(target_os = "windows")]
    if target_exe.is_some_and(tyl_platform::backend::is_denied_terminal) {
        return false;
    }
    let _ = target_exe;
    editable != Some(false)
}

pub fn remember(
    request_id: u64,
    target: TargetWindow,
    original: &str,
    editable: bool,
    selection_id: Option<Vec<i32>>,
) {
    *SELECTION.lock().unwrap() = Some(Selection {
        request_id,
        target,
        original: original.into(),
        editable,
        selection_id,
    });
}

pub fn invalidate() {
    *SELECTION.lock().unwrap() = None;
}

pub fn is_current(request_id: u64) -> bool {
    SELECTION
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|s| s.request_id == request_id)
}

fn matches_original(selection: &Selection, identity: Option<&[i32]>, text: &str) -> bool {
    (selection.selection_id.is_none() || selection.selection_id.as_deref() == identity)
        && !text.trim().is_empty()
        && selection.original == text
}

#[cfg(target_os = "windows")]
pub use native::{copy, foreground, replace};

#[cfg(not(target_os = "windows"))]
pub fn foreground() -> TargetWindow {
    TargetWindow::default()
}
#[cfg(not(target_os = "windows"))]
pub fn copy(_owner: usize, _text: &str) -> Result<(), String> {
    Err("当前平台暂不支持复制".into())
}
#[cfg(not(target_os = "windows"))]
pub fn replace(_owner: usize, _request_id: u64, _text: &str) -> Result<(), String> {
    Err("当前平台暂不支持替换".into())
}

#[cfg(target_os = "windows")]
mod native {
    use super::*;
    use std::time::{Duration, Instant};
    use tyl_platform::backend::uia_selection::{
        focused_control_editable, focused_selection, restore_unique_selection, selection_editable,
        selection_identity,
    };
    use windows::core::Interface;
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, IUIAutomation2};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_V,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, IsWindow,
        SetForegroundWindow, GUITHREADINFO,
    };

    pub fn foreground() -> TargetWindow {
        unsafe {
            let window = GetForegroundWindow();
            let mut process_id = 0;
            let thread_id = GetWindowThreadProcessId(window, Some(&mut process_id));
            TargetWindow {
                handle: window.0 as usize,
                process_id,
                focus_handle: focused_hwnd(thread_id).unwrap_or_default(),
            }
        }
    }

    fn focused_hwnd(thread_id: u32) -> Option<usize> {
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        unsafe { GetGUIThreadInfo(thread_id, &mut info) }.ok()?;
        (!info.hwndFocus.0.is_null()).then_some(info.hwndFocus.0 as usize)
    }

    fn focus_matches(target: TargetWindow) -> bool {
        unsafe {
            let hwnd = HWND(target.handle as *mut _);
            if GetForegroundWindow() != hwnd {
                return false;
            }
            let thread_id = GetWindowThreadProcessId(hwnd, None);
            target.focus_handle == 0 || focused_hwnd(thread_id) == Some(target.focus_handle)
        }
    }

    pub fn copy(owner: usize, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Err("还没有可复制的译文".into());
        }
        let units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2).map_err(|e| e.to_string())?;
            let pointer = GlobalLock(memory) as *mut u16;
            if pointer.is_null() {
                let _ = GlobalFree(Some(memory));
                return Err("无法分配剪贴板内存".into());
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), pointer, units.len());
            let _ = GlobalUnlock(memory);
            let mut opened = false;
            for _ in 0..15 {
                if OpenClipboard(Some(HWND(owner as *mut _))).is_ok() {
                    opened = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            if !opened {
                let _ = GlobalFree(Some(memory));
                return Err("剪贴板正被占用，请重试".into());
            }
            let result = EmptyClipboard()
                .and_then(|()| SetClipboardData(13, Some(HANDLE(memory.0))).map(|_| ()));
            let _ = CloseClipboard();
            if result.is_err() {
                let _ = GlobalFree(Some(memory));
            }
            result.map_err(|_| "复制失败，请重试".into())
        }
    }

    struct OleGuard;
    impl Drop for OleGuard {
        fn drop(&mut self) {
            unsafe {
                OleUninitialize();
            }
        }
    }

    enum VerifyFailure {
        ReadOnly,
        Unavailable(String),
    }

    fn verify_selection(uia: &IUIAutomation, original: &Selection) -> Result<(), VerifyFailure> {
        let current = focused_selection(uia).map_err(VerifyFailure::Unavailable)?;
        if current.count != 1 {
            return Err(VerifyFailure::Unavailable(
                "请保持一段文本处于选中状态".into(),
            ));
        }
        if !matches_original(
            original,
            selection_identity(&current).as_deref(),
            &current.text,
        ) {
            return Err(VerifyFailure::Unavailable(
                "原选区或输入控件已变化，请重新划词".into(),
            ));
        }
        match selection_editable(uia, &current) {
            Some(true) => Ok(()),
            Some(false) => Err(VerifyFailure::ReadOnly),
            // Exact UIA text plus the caller's captured native focus binding
            // can verify a selection even when writability metadata is absent.
            None => Ok(()),
        }
    }

    pub fn replace(owner: usize, request_id: u64, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Err("还没有可替换的译文".into());
        }
        let selection = SELECTION
            .lock()
            .unwrap()
            .clone()
            .filter(|s| s.request_id == request_id)
            .ok_or("选区已过期，请重新划词")?;
        let target = selection.target;
        if !selection.editable {
            return Err("原文不可编辑，请使用复制译文".into());
        }
        unsafe {
            OleInitialize(None).map_err(|_| "无法连接原编辑器")?;
            let _ole = OleGuard;
            let hwnd = HWND(target.handle as *mut _);
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if !IsWindow(Some(hwnd)).as_bool() || pid != target.process_id {
                return Err("原窗口已关闭，请重新划词".into());
            }
            let _ = SetForegroundWindow(hwnd);
            let deadline = Instant::now() + Duration::from_millis(400);
            while GetForegroundWindow() != hwnd && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(15));
            }
            if !focus_matches(target) {
                return Err("无法返回原窗口，请复制译文后粘贴".into());
            }
            // Native activation precedes framework focus-in/selection updates.
            // Allow the target's input queue to settle before reading/copying.
            let settle_until = Instant::now() + Duration::from_millis(150);
            while Instant::now() < settle_until {
                if !focus_matches(target) || !is_current(request_id) {
                    return Err("焦点已离开原输入控件，请重新划词".into());
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            if [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
                .iter()
                .any(|key| GetAsyncKeyState(i32::from(key.0)) < 0)
            {
                return Err("请松开键盘修饰键后再点击替换".into());
            }

            // A browser's UIA provider may live in a renderer process, not the
            // HWND owner process. Bind to the captured RuntimeId + exact text
            // instead of comparing unrelated process IDs. UIA focus can lag
            // behind native activation; allow a bounded settle period.
            let uia;
            let mut uia_verified = false;
            {
                let instance: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                        .map_err(|e| e.to_string())?;
                if let Ok(uia2) = instance.cast::<IUIAutomation2>() {
                    let _ = uia2.SetConnectionTimeout(400);
                    let _ = uia2.SetTransactionTimeout(800);
                }
                let settle_until = Instant::now() + Duration::from_millis(700);
                loop {
                    if !focus_matches(target) || !is_current(request_id) {
                        return Err("焦点已离开原窗口，请重新划词".into());
                    }
                    match verify_selection(&instance, &selection) {
                        Ok(()) => {
                            uia_verified = true;
                            break;
                        }
                        Err(VerifyFailure::ReadOnly) => {
                            return Err("原文不可编辑，请使用复制译文".into());
                        }
                        Err(VerifyFailure::Unavailable(error))
                            if Instant::now() >= settle_until =>
                        {
                            crate::logger::log_str(&format!(
                                "[replace] UIA 复核不可用，改用通用选区回读: {error}"
                            ));
                            break;
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(25)),
                    }
                }
                if !uia_verified
                    && restore_unique_selection(
                        &instance,
                        &selection.original,
                        selection.selection_id.as_deref(),
                        || focus_matches(target) && is_current(request_id),
                    )
                    .is_ok()
                {
                    uia_verified = verify_selection(&instance, &selection).is_ok();
                    crate::logger::log_str("[replace] 已恢复原编辑器的唯一匹配选区");
                }
                uia = instance;
            }
            // No stable RuntimeId, or the provider stopped exposing it after
            // focus returned: re-copy and compare the exact selected text.
            // This is capability-driven and works across rich editors without
            // application-name allowlists.
            let clipboard_guard = if uia_verified {
                None
            } else {
                crate::logger::log_str("[replace] 使用通用富文本选区回读");
                // Recheck explicit read-only/password/disabled metadata if the
                // provider has become available since the original capture.
                if let Ok(instance) =
                    CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                {
                    if let Ok(uia2) = instance.cast::<IUIAutomation2>() {
                        let _ = uia2.SetConnectionTimeout(200);
                        let _ = uia2.SetTransactionTimeout(300);
                    }
                    if focused_control_editable(&instance) == Some(false) {
                        return Err("原文不可编辑，请使用复制译文".into());
                    }
                }
                if !focus_matches(target) || !is_current(request_id) {
                    return Err("焦点已离开原输入控件，请重新划词".into());
                }
                Some(
                    tyl_platform::backend::clipboard_fallback::verify_selected_text(
                        owner,
                        &selection.original,
                        Duration::from_millis(1200),
                        || {
                            focus_matches(target)
                                && is_current(request_id)
                                && ![VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
                                    .iter()
                                    .any(|key| GetAsyncKeyState(i32::from(key.0)) < 0)
                        },
                    )?,
                )
            };
            // Do not inject into a new focus or while physical modifier keys are held.
            if !focus_matches(target)
                || SELECTION.lock().unwrap().as_ref().map(|s| s.request_id) != Some(request_id)
            {
                return Err("选区已变化，请重新划词".into());
            }
            if [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
                .iter()
                .any(|key| GetAsyncKeyState(i32::from(key.0)) < 0)
            {
                return Err("请松开键盘修饰键后再点击替换".into());
            }
            let clipboard_guard =
                tyl_platform::backend::clipboard_fallback::prepare_hidden_replacement(
                    owner,
                    text,
                    clipboard_guard,
                )?;
            // Clipboard listeners can change focus or selection. Verify again
            // after copying, including exact text, before injecting any keys.
            if uia_verified {
                verify_selection(&uia, &selection).map_err(|_| "原选区已变化，请重新划词")?;
            }
            if !focus_matches(target)
                || !is_current(request_id)
                || [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
                    .iter()
                    .any(|key| GetAsyncKeyState(i32::from(key.0)) < 0)
            {
                return Err("焦点已变化，请重新划词".into());
            }
            let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        dwFlags: if up {
                            KEYEVENTF_KEYUP
                        } else {
                            Default::default()
                        },
                        ..Default::default()
                    },
                },
            };
            let sequence = [
                key(VK_CONTROL, false),
                key(VK_V, false),
                key(VK_V, true),
                key(VK_CONTROL, true),
            ];
            if SendInput(&sequence, std::mem::size_of::<INPUT>() as i32) != sequence.len() as u32 {
                let releases = [key(VK_V, true), key(VK_CONTROL, true)];
                SendInput(&releases, std::mem::size_of::<INPUT>() as i32);
                return Err("编辑器拒绝了粘贴，请重试".into());
            }
            // SendInput only queues keystrokes. Keep the hidden payload alive
            // briefly so richer editors have time to consume Ctrl+V, then put
            // the user's original clipboard back if nobody else changed it.
            std::thread::sleep(Duration::from_millis(180));
            let _ = clipboard_guard.restore();
            // One click only: another replacement requires a fresh capture.
            let mut current = SELECTION.lock().unwrap();
            if current.as_ref().is_some_and(|s| s.request_id == request_id) {
                *current = None;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_editability_allows_explicit_attempt_but_readonly_does_not() {
        assert!(can_offer_replacement(None, Some("custom-editor")));
        assert!(can_offer_replacement(Some(true), Some("custom-editor")));
        assert!(!can_offer_replacement(Some(false), Some("custom-editor")));
        #[cfg(target_os = "windows")]
        for exe in ["pwsh", "WindowsTerminal", "conhost"] {
            assert!(!can_offer_replacement(None, Some(exe)));
            assert!(!can_offer_replacement(Some(true), Some(exe)));
        }
    }

    #[test]
    fn replacement_requires_the_same_control_and_exact_selection() {
        let selection = Selection {
            request_id: 1,
            target: TargetWindow::default(),
            original: "selected  text".into(),
            editable: true,
            selection_id: Some(vec![1, 2, 3]),
        };
        assert!(matches_original(
            &selection,
            Some(&[1, 2, 3]),
            "selected  text"
        ));
        assert!(!matches_original(
            &selection,
            Some(&[1, 2, 4]),
            "selected  text"
        ));
        assert!(!matches_original(&selection, None, "selected  text"));
        assert!(!matches_original(
            &selection,
            Some(&[1, 2, 3]),
            "selected text"
        ));
        assert!(!matches_original(&selection, Some(&[1, 2, 3]), ""));
    }
}
