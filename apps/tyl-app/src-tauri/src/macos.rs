use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;
use serde::Serialize;

pub fn copy_translation(text: &str) -> Result<(), String> {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    if pasteboard.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString }) {
        Ok(())
    } else {
        Err("复制失败，请重试".into())
    }
}

pub fn compact_menu_bar_item(tray: &tauri::tray::TrayIcon) -> tauri::Result<()> {
    tray.with_inner_tray_icon(|icon| {
        if let Some(item) = icon.ns_status_item() {
            // Match the template image's 18-point width on notched displays.
            item.setLength(18.0);
        }
    })
}

#[derive(Serialize)]
pub struct CaptureStatus {
    registered: Option<bool>,
    accessibility: bool,
}

#[tauri::command]
pub fn mac_capture_status(app: tauri::AppHandle) -> CaptureStatus {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
    let cfg = crate::settings::current();
    let registered = (cfg.trigger_mode == crate::settings::TriggerMode::Hotkey).then(|| {
        cfg.hotkey
            .parse::<Shortcut>()
            .is_ok_and(|key| app.global_shortcut().is_registered(key))
    });
    CaptureStatus {
        registered,
        accessibility: tyl_platform::backend::accessibility_trusted(),
    }
}

#[tauri::command]
pub fn open_accessibility_settings() -> Result<(), String> {
    tyl_platform::backend::request_accessibility();
    open_system_settings("com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility")
}

#[tauri::command]
pub fn open_menu_bar_settings() -> Result<(), String> {
    open_system_settings("com.apple.ControlCenter-Settings.extension")
}

fn open_system_settings(pane: &str) -> Result<(), String> {
    let status = std::process::Command::new("/usr/bin/open")
        .arg(format!("x-apple.systempreferences:{pane}"))
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("Could not open System Settings".into())
    }
}
