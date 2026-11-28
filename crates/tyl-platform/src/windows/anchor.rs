//! Anchor snapshot: cursor position + foreground process name, taken before
//! any side-effecting capture work.

use tyl_core::CaptureAnchor;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Threading::OpenProcess;
use windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId,
};

/// Declares per-monitor-v2 DPI awareness. MUST run before any coordinate or
/// DPI query, or Windows virtualizes everything to 96 DPI (lying monitors,
/// scaled-down cursor coords, wrong popup placement). Safe to call multiple
/// times; subsequent calls fail harmlessly.
pub fn enable_dpi_awareness() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn capture_anchor() -> CaptureAnchor {
    let cursor = cursor_pos().map(|p| tyl_core::locator::Point { x: p.x, y: p.y });
    let target_exe = foreground_process_name();
    CaptureAnchor { cursor, target_exe }
}

fn cursor_pos() -> Option<POINT> {
    let mut pt = POINT::default();
    unsafe { GetCursorPos(&mut pt) }.ok()?;
    Some(pt)
}

fn foreground_process_name() -> Option<String> {
    let hwnd: HWND = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    // PROCESS_QUERY_LIMITED_INFORMATION works across integrity levels better
    // than QUERY_INFORMATION|VM_READ.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let name = std::path::Path::new(&process_image_name(process)?)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase());
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(process) };
    name
}

fn process_image_name(process: windows::Win32::Foundation::HANDLE) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};

    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    }
    .ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// Native selection evidence for providers with missing or nonfunctional TextPattern.
pub(super) fn native_text_selection(point: tyl_core::locator::Point) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetGUIThreadInfo, WindowFromPoint, GUITHREADINFO,
    };
    unsafe {
        let thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        thread != 0
            && GetGUIThreadInfo(thread, &mut info).is_ok()
            && WindowFromPoint(POINT {
                x: point.x,
                y: point.y,
            }) == info.hwndFocus
            && edit_has_selection(info.hwndFocus)
    }
}

fn edit_has_selection(edit: HWND) -> bool {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::{
            Controls::{EM_GETPASSWORDCHAR, EM_GETSEL},
            WindowsAndMessaging::*,
        },
    };
    unsafe {
        let mut class = [0u16; 64];
        let len = GetClassNameW(edit, &mut class);
        let class = String::from_utf16_lossy(&class[..len as usize]).to_ascii_lowercase();
        if !(class == "edit" || class.starts_with("richedit"))
            || GetWindowLongW(edit, GWL_STYLE) & ES_PASSWORD != 0
        {
            return false;
        }
        let query = |msg| {
            let mut value = 0;
            (SendMessageTimeoutW(
                edit,
                msg,
                WPARAM(0),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                40,
                Some(&mut value),
            )
            .0 != 0)
                .then_some(value)
        };
        query(EM_GETPASSWORDCHAR) == Some(0)
            && query(EM_GETSEL).is_some_and(|range| range as u16 != (range >> 16) as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::{
        core::w,
        Win32::{
            Foundation::{LPARAM, WPARAM},
            UI::{Controls::EM_SETSEL, WindowsAndMessaging::*},
        },
    };
    #[test]
    fn native_fallback_requires_selected_text_and_skips_password_edits() {
        unsafe {
            for protected in [false, true] {
                let style = if protected {
                    WINDOW_STYLE(ES_PASSWORD as u32)
                } else {
                    WINDOW_STYLE::default()
                };
                let edit = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("Edit"),
                    w!("sample"),
                    style,
                    0,
                    0,
                    80,
                    20,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
                SendMessageW(edit, EM_SETSEL, Some(WPARAM(2)), Some(LPARAM(2)));
                assert!(!edit_has_selection(edit));
                SendMessageW(edit, EM_SETSEL, Some(WPARAM(1)), Some(LPARAM(4)));
                assert_eq!(edit_has_selection(edit), !protected);
                DestroyWindow(edit).unwrap();
            }
        }
    }
}
