//! Registration and mode changes run on the event loop that owns the native mouse monitor.
use crate::{
    pipeline::Pipeline,
    selection_monitor,
    settings::{self, Settings, TriggerMode},
};
use std::{cell::RefCell, sync::Arc};
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

enum Active {
    Hotkey(Shortcut),
    Automatic {
        _monitor: selection_monitor::Monitor,
    },
}
thread_local! {static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };}

pub fn configure(app: &AppHandle, cfg: &Settings) -> Result<(), String> {
    let pipeline = app.state::<Arc<Pipeline>>();
    ACTIVE.with_borrow_mut(|active| {
        let next = match cfg.trigger_mode {
            TriggerMode::Automatic => {
                if matches!(active, Some(Active::Automatic { .. })) {
                    return Ok(());
                }
                Active::Automatic {
                    _monitor: selection_monitor::install(app, Arc::clone(&pipeline))?,
                }
            }
            TriggerMode::Hotkey => {
                let key = cfg.hotkey.parse::<Shortcut>().map_err(|_| {
                    crate::i18n::text(&cfg.language, "快捷键格式无效", "Invalid keyboard shortcut")
                        .to_owned()
                })?;
                if matches!(active, Some(Active::Hotkey(old)) if *old == key) {
                    return Ok(());
                }
                app.global_shortcut()
                    .on_shortcut(key, crate::capture_shortcut_handler(Arc::clone(&pipeline)))
                    .map_err(|e| {
                        format!(
                            "{} ({e})",
                            crate::i18n::text(
                                &cfg.language,
                                "无法注册此快捷键，请换一个组合",
                                "Could not register this shortcut. Try another combination."
                            )
                        )
                    })?;
                Active::Hotkey(key)
            }
        };
        if let Some(Active::Hotkey(old)) = active {
            if let Err(e) = app.global_shortcut().unregister(*old) {
                if let Active::Hotkey(key) = next {
                    let _ = app.global_shortcut().unregister(key);
                }
                return Err(e.to_string());
            }
        }
        pipeline.invalidate();
        *active = Some(next);
        if let Some(w) = app.get_webview_window("popup") {
            if w.is_visible().unwrap_or(false) || crate::window_ctl::passive_visible() {
                crate::window_ctl::hide(&w);
            }
        }
        Ok(())
    })
}

pub fn save(app: &AppHandle, cfg: Settings) -> Result<(), String> {
    let old = settings::current();
    configure(app, &cfg)?;
    if let Err(e) = settings::save(cfg) {
        configure(app, &old).map_err(|restore| format!("{e} ({restore})"))?;
        return Err(e);
    }
    crate::refresh_tray_i18n(app);
    Ok(())
}
