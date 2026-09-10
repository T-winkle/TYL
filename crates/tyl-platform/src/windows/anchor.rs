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
