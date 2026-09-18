//! TYL Tauri 壳：常驻隐藏弹窗 + 全局热键 + 取词管线 + 设置。
//!
//! 交互模型（M2 定稿，见 window_ctl 模块注释）：弹窗显示即聚焦，
//! 失焦隐藏是唯一主路径——简单、与 Windows 焦点模型合作而非对抗。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod desktop_actions;
mod gpu;
mod i18n;
mod logger;
#[cfg(feature = "memory-bench")]
mod memory_bench;
mod pipeline;
mod platform_capture;
mod runtime;
mod settings;
mod translate;
#[cfg(feature = "memory-bench")]
mod visual_bench;
mod webview_memory;
mod window_ctl;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

/// 前端就绪标志（就绪前管线不推送事件，避免丢首批取词）。
pub struct ReadyFlag(pub AtomicBool);

/// One physical hotkey gesture must produce at most one capture request.
///
/// Windows emits repeated `WM_HOTKEY` messages while the shortcut is held.
/// `tauri-plugin-global-shortcut` exposes each one as `Pressed`, so treating
/// every callback as a new gesture starts overlapping clipboard transactions.
/// The matching `Released` event is used only to re-arm the next gesture.
#[derive(Default)]
struct HotkeyGestureGate {
    pressed: AtomicBool,
}

impl HotkeyGestureGate {
    /// Returns true only for the first `Pressed` event in a gesture.
    fn begin_press(&self) -> bool {
        self.pressed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Returns true when this event actually re-armed a pressed gesture.
    fn release(&self) -> bool {
        self.pressed.swap(false, Ordering::AcqRel)
    }
}

fn main() {
    // Load the logging threshold before producing startup diagnostics (Off too).
    let cfg = settings::load();
    #[cfg(feature = "memory-bench")]
    let cfg = memory_bench::rendering_config(cfg);
    logger::set_level(cfg.log_level);
    // 日志最先启动：之后每一步都有迹可循（release 无控制台）。
    logger::init();
    let context = tauri::generate_context!();
    #[cfg(feature = "memory-bench")]
    let context = memory_bench::configure(context);

    // 平台层（剪贴板通道）日志桥接到 tyl.log——GUI 无 stderr 可看。
    #[cfg(target_os = "windows")]
    tyl_platform::backend::clipboard_fallback::set_log_hook(logger::trace);

    // DPI 感知必须最先声明（Windows），否则所有坐标被虚拟化。
    tyl_platform::backend::enable_dpi_awareness();
    logger::log_str("DPI 感知已声明");

    // 设置加载（热键/LLM/代理/策略）。
    gpu::initialize(cfg.gpu_acceleration);
    logger::info(&format!(
        "设置已加载: 热键={} 代理模式={} AI引擎={}",
        cfg.hotkey,
        cfg.proxy.mode,
        cfg.engines.iter().any(|e| e == "llm")
    ));

    // 预热：COM/OLE/UIA 惰性初始化前置到启动期（防首次取词慢+快照缺格式）。
    platform_capture::warmup();
    logger::log_str("预热已启动");

    let pipeline = Arc::new(pipeline::Pipeline::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // 单实例：二次启动时唤起已有实例的设置窗口（并退出新进程）。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            logger::log_str("第二实例启动：唤起设置窗口");
            open_settings_window(app);
        }))
        // 开机自启：设置开关经 commands 控制（ManagerExt）。
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(ReadyFlag(AtomicBool::new(false)))
        .invoke_handler(tauri::generate_handler![
            commands::hide_popup,
            commands::translate_one,
            commands::fit_popup_height,
            commands::speak,
            commands::pronunciation_audio,
            commands::copy_translation,
            commands::replace_selection,
            commands::get_settings,
            commands::test_llm,
            gpu::get_gpu_status,
            commands::save_settings,
            commands::get_autostart,
            commands::set_autostart,
            commands::frontend_ready,
            commands::frontend_log
        ])
        // 页面加载生命周期打点：WebView 重建等异常可在此发现
        .on_page_load(|webview, payload| match payload.event() {
            tauri::webview::PageLoadEvent::Started => {
                logger::log_str(&format!(
                    "页面加载开始: {} ({})",
                    payload.url(),
                    webview.label()
                ));
            }
            _ => {
                logger::log_str(&format!(
                    "页面加载完成: {} ({})",
                    payload.url(),
                    webview.label()
                ));
            }
        })
        .setup({
            let pipeline = Arc::clone(&pipeline);
            move |app| {
                // 弹窗样式 patch（TOOLWINDOW；不进 Alt+Tab）
                if let Some(w) = app.get_webview_window("popup") {
                    window_ctl::patch_window_style(&w);
                } else {
                    logger::error("错误: popup 窗口不存在");
                }

                // 托盘：设置 + 退出。
                let quit_app = app.handle().clone();
                let settings_app = app.handle().clone();
                let language = settings::current().language;
                let mut tray = TrayIconBuilder::with_id("main")
                    .tooltip(i18n::text(
                        &language,
                        "TYL 划词翻译（右键菜单）",
                        "TYL Selection Translator (right-click for menu)",
                    ))
                    .menu(&build_tray_menu(app.handle(), &language)?)
                    .show_menu_on_left_click(false)
                    .on_menu_event(move |_app, event| match event.id.as_ref() {
                        "quit" => {
                            logger::log_str("退出");
                            quit_app.exit(0);
                            std::process::exit(0);
                        }
                        "settings" => {
                            logger::log_str("菜单: 打开设置");
                            open_settings_window(&settings_app);
                        }
                        _ => {}
                    });
                if let Some(icon) = app.default_window_icon() {
                    tray = tray.icon(icon.clone());
                }
                tray.build(app)?;
                logger::log_str("托盘已创建（右键弹菜单）");

                // 全局热键（设置里的值）→ 管线。
                let hotkey_app = app.handle().clone();
                let hotkey_pipeline = Arc::clone(&pipeline);
                let hotkey_gesture = Arc::new(HotkeyGestureGate::default());
                app.global_shortcut().on_shortcut(
                    settings::current().hotkey.as_str(),
                    move |_app, _sc, event| match event.state() {
                        ShortcutState::Pressed => {
                            if !hotkey_gesture.begin_press() {
                                logger::trace("热键仍处于按下状态：已忽略系统自动重复");
                                return;
                            }
                            if !hotkey_app.state::<ReadyFlag>().0.load(Ordering::SeqCst) {
                                logger::log_str("热键触发，但前端尚未就绪；已跳过本次取词");
                                return;
                            }
                            logger::log_str("热键触发");
                            pipeline::Pipeline::on_hotkey(
                                hotkey_app.clone(),
                                Arc::clone(&hotkey_pipeline),
                            );
                        }
                        ShortcutState::Released => {
                            if hotkey_gesture.release() {
                                logger::trace("热键已释放：下一次取词已重新启用");
                            }
                        }
                    },
                )?;
                logger::log_str(&format!("热键 {} 已注册", settings::current().hotkey));

                #[cfg(feature = "memory-bench")]
                memory_bench::start(app.handle().clone(), Arc::clone(&pipeline));

                Ok(())
            }
        })
        .on_window_event(|window, event| {
            let label = window.label();
            // popup：失焦即隐藏（唯一主路径）
            if label == "popup" {
                if let tauri::WindowEvent::Focused(focused) = event {
                    if let Some(popup) = window.app_handle().get_webview_window("popup") {
                        window_ctl::focus_changed(&popup, *focused);
                    }
                }
            }
            // settings：关闭即销毁（下次从托盘重新建，读最新配置）
            if label == "settings" {
                if let tauri::WindowEvent::Destroyed = event {
                    window.app_handle().emit("tyl://settings-closed", ()).ok();
                }
            }
        })
        .run(context)
        .expect("error while running tyl-app");
}

/// 打开设置窗口（已存在则聚焦）。
pub fn open_settings_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let language = settings::current().language;
    let _ = tauri::WebviewWindowBuilder::new(
        app,
        "settings",
        tauri::WebviewUrl::App("/settings.html".into()),
    )
    .title(i18n::text(&language, "TYL 设置", "TYL Settings"))
    .decorations(false)
    .shadow(true)
    .inner_size(960.0, 740.0)
    .min_inner_size(760.0, 600.0)
    .resizable(true)
    .minimizable(false)
    .center()
    .build();
}

fn build_tray_menu(
    app: &tauri::AppHandle,
    language: &str,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    tauri::menu::MenuBuilder::new(app)
        .text("settings", i18n::text(language, "设置…", "Settings…"))
        .separator()
        .text("quit", i18n::text(language, "退出", "Quit"))
        .build()
}

pub(crate) fn refresh_tray_i18n(app: &tauri::AppHandle) {
    let language = settings::current().language;
    let Some(tray) = app.tray_by_id("main") else {
        return;
    };
    if let Ok(menu) = build_tray_menu(app, &language) {
        let _ = tray.set_menu(Some(menu));
    }
    let _ = tray.set_tooltip(Some(i18n::text(
        &language,
        "TYL 划词翻译（右键菜单）",
        "TYL Selection Translator (right-click for menu)",
    )));
}

#[cfg(test)]
mod hotkey_tests {
    use super::HotkeyGestureGate;

    #[test]
    fn one_capture_is_admitted_per_physical_gesture() {
        let gate = HotkeyGestureGate::default();

        assert!(gate.begin_press());
        assert!(!gate.begin_press());
        assert!(!gate.begin_press());
        assert!(gate.release());
        assert!(!gate.release());
        assert!(gate.begin_press());
    }
}
