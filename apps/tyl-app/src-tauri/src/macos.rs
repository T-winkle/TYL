use std::sync::{Arc, Mutex};

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;
use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use crate::{i18n, logger, pipeline::Pipeline, settings};

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

struct HotkeyRegistration {
    active: Mutex<Option<Shortcut>>,
    pipeline: Arc<Pipeline>,
}

impl HotkeyRegistration {
    fn register(
        &self,
        app: &AppHandle,
        shortcut: Shortcut,
    ) -> Result<(), tauri_plugin_global_shortcut::Error> {
        app.global_shortcut().on_shortcut(
            shortcut,
            crate::capture_shortcut_handler(Arc::clone(&self.pipeline)),
        )
    }
}

pub fn install_hotkey(app: &AppHandle, pipeline: Arc<Pipeline>) {
    let registration = HotkeyRegistration {
        active: Mutex::new(None),
        pipeline,
    };
    let hotkey = settings::current().hotkey;
    match hotkey.parse::<Shortcut>() {
        Ok(shortcut) => match registration.register(app, shortcut) {
            Ok(()) => {
                *registration.active.lock().unwrap() = Some(shortcut);
                logger::info(&format!("[macOS] 全局热键 {hotkey} 已注册"));
            }
            Err(error) => logger::warn(&format!("[macOS] 注册全局热键 {hotkey} 失败: {error}")),
        },
        Err(error) => logger::warn(&format!("[macOS] 全局热键格式无效: {error}")),
    }
    app.manage(registration);
    logger::info(&format!(
        "[macOS] 辅助功能已授权: {}",
        tyl_platform::backend::accessibility_trusted()
    ));
}

pub fn save_with_hotkey(app: &AppHandle, settings: settings::Settings) -> Result<(), String> {
    let language = &settings.language;
    let registration = app.state::<HotkeyRegistration>();
    let mut active = registration.active.lock().unwrap();
    let next = settings.hotkey.parse::<Shortcut>().map_err(|_| {
        i18n::text(language, "快捷键格式无效", "Invalid keyboard shortcut").to_string()
    })?;
    if *active == Some(next) {
        return settings::save(settings);
    }

    registration.register(app, next).map_err(|error| {
        logger::warn(&format!(
            "[macOS] 注册全局热键 {} 失败: {error}",
            settings.hotkey
        ));
        i18n::text(
            language,
            "无法注册此快捷键，请换一个组合",
            "Could not register this shortcut. Try a different combination.",
        )
        .to_string()
    })?;

    if let Some(old) = *active {
        if let Err(error) = app.global_shortcut().unregister(old) {
            unregister_for_rollback(app, next);
            return Err(error.to_string());
        }
    }

    if let Err(error) = settings::save(settings) {
        unregister_for_rollback(app, next);
        if let Some(old) = *active {
            if let Err(error) = registration.register(app, old) {
                logger::error(&format!("[macOS] 恢复原全局热键失败: {error}"));
                *active = None;
            }
        }
        return Err(error);
    }

    *active = Some(next);
    logger::info(&format!(
        "[macOS] 全局热键 {} 已生效",
        settings::current().hotkey
    ));
    Ok(())
}

fn unregister_for_rollback(app: &AppHandle, shortcut: Shortcut) {
    if let Err(error) = app.global_shortcut().unregister(shortcut) {
        logger::error(&format!("[macOS] 撤销新全局热键失败: {error}"));
    }
}

#[derive(Serialize)]
pub struct CaptureStatus {
    registered: bool,
    accessibility: bool,
}

#[tauri::command]
pub fn mac_capture_status(app: AppHandle) -> CaptureStatus {
    let registration = app.state::<HotkeyRegistration>();
    let registered = registration.active.lock().unwrap().is_some();
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
