//! Monitor enumeration via Win32 (EnumDisplayMonitors + GetMonitorInfoW).

use tyl_core::locator::{MonitorInfo, Rect, ScreenLocator};

use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

pub struct WinLocator;

impl WinLocator {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WinLocator {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenLocator for WinLocator {
    fn monitors(&self) -> Vec<MonitorInfo> {
        let mut out: Vec<MonitorInfo> = Vec::new();
        let lparam = LPARAM(&mut out as *mut _ as *mut core::ffi::c_void as isize);
        unsafe {
            // Safety: the callback only pushes into the Vec behind `lparam`;
            // EnumDisplayMonitors doesn't return until the walk finishes.
            let _ = EnumDisplayMonitors(None, None, Some(monitor_callback), lparam);
        }
        out
    }
}

fn to_rect(r: RECT) -> Rect {
    Rect {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

unsafe extern "system" fn monitor_callback(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let out = &mut *(lparam.0 as *mut Vec<MonitorInfo>);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(hmonitor, &mut info).as_bool() {
        let scale = effective_dpi_scale(hmonitor);
        out.push(MonitorInfo {
            rect: to_rect(info.rcMonitor),
            work: to_rect(info.rcWork),
            scale,
        });
    }
    BOOL(1)
}

fn effective_dpi_scale(hmonitor: HMONITOR) -> f64 {
    unsafe {
        let mut dpi_x = 0u32;
        let mut dpi_y = 0u32;
        if GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_ok() {
            dpi_x as f64 / 96.0
        } else {
            1.0
        }
    }
}
