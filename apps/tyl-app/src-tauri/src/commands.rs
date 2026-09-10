//! 前端可调用的 Tauri commands。

use tauri::{AppHandle, Emitter, Manager};

#[tauri::command]
pub fn hide_popup(app: AppHandle) {
    crate::logger::log_str("前端请求: hide_popup");
    if let Some(w) = app.get_webview_window("popup") {
        crate::window_ctl::hide(&w);
    }
}

/// 懒加载单引擎翻译（弹窗切 tab 时调用）。结果走 tyl://translate 事件
/// 回推（service=engine），前端按 service 归位。
#[tauri::command(rename_all = "snake_case")]
pub fn translate_one(app: AppHandle, text: String, engine: String, request_id: u64) {
    if !crate::pipeline::claim_engine(request_id, &engine) {
        return;
    }
    let rt = crate::runtime::handle().clone();
    std::thread::spawn(move || {
        let target = crate::translate::auto_target(&text).to_string();
        emit_translation(&app, request_id, "start", String::new(), &engine);

        if engine == crate::translate::google::ENGINE_LLM {
            let cfg = crate::settings::current();
            if cfg.llm.api_key.trim().is_empty() {
                emit_translation(
                    &app,
                    request_id,
                    "error",
                    "AI 引擎未配置 API Key".into(),
                    &engine,
                );
                return;
            }
            let llm_cfg = crate::translate::llm::LlmConfig {
                base_url: cfg.llm.base_url,
                api_key: cfg.llm.api_key,
                model: cfg.llm.model,
            };
            let chunk_app = app.clone();
            let chunk_engine = engine.clone();
            let mut on_chunk = move |chunk: String| {
                emit_translation(&chunk_app, request_id, "chunk", chunk, &chunk_engine);
            };
            match rt.block_on(crate::translate::llm::translate_stream(
                &llm_cfg,
                &text,
                &target,
                &mut on_chunk,
            )) {
                Ok(full) if !full.trim().is_empty() => {
                    emit_translation(&app, request_id, "done", full, &engine);
                }
                Ok(_) => {
                    emit_translation(&app, request_id, "error", "empty".into(), &engine);
                }
                Err(error) => {
                    crate::logger::warn(&format!("[translate/llm] request={request_id} {error}"));
                    emit_translation(&app, request_id, "error", error, &engine);
                }
            }
            return;
        }

        match rt.block_on(crate::pipeline::run_engine(&engine, &text, &target)) {
            Ok(translated) if !translated.trim().is_empty() => {
                emit_translation(&app, request_id, "done", translated, &engine);
            }
            Ok(_) => emit_translation(&app, request_id, "error", "empty".into(), &engine),
            Err(error) => emit_translation(&app, request_id, "error", error, &engine),
        }
    });
}

fn emit_translation(
    app: &AppHandle,
    request_id: u64,
    phase: &'static str,
    text: String,
    service: &str,
) {
    let _ = app.emit(
        crate::pipeline::TRANSLATE_EVENT,
        crate::pipeline::TranslateEvent {
            request_id,
            phase,
            text,
            service: service.into(),
        },
    );
}

/// 弹窗高度自适应（前端量完整卡片自然高度后调用）。
/// height 是逻辑像素。窗口最多占工作区 86%（且不超过 940px），超出后内容区滚动；
/// 初始位于选区上方时保持底边，否则保持顶边，并始终夹在工作区内。
#[tauri::command(rename_all = "snake_case")]
pub fn fit_popup_height(app: AppHandle, height: f64, viewport_width: f64, grow_upward: bool) {
    let Some(w) = app.get_webview_window("popup") else {
        return;
    };
    if !height.is_finite() || height <= 0.0 {
        return;
    }
    let (Ok(position), Ok(size)) = (w.outer_position(), w.outer_size()) else {
        return;
    };

    let (monitors, _) = crate::platform_capture::monitors();
    if monitors.is_empty() {
        return;
    }
    let center = tyl_core::locator::Point {
        x: position.x + i32::try_from(size.width / 2).unwrap_or(i32::MAX),
        y: position.y + i32::try_from(size.height / 2).unwrap_or(i32::MAX),
    };
    let monitor = *tyl_core::locator::pick_monitor(&monitors, center);
    let scale = monitor.scale.max(0.1);
    // CSS pixels can differ from native logical pixels (WebView zoom / mixed DPI).
    // Map the measured DOM height using the actual physical-to-CSS viewport ratio.
    let content_width = w.inner_size().map(|s| s.width).unwrap_or(size.width);
    let natural_height = css_to_logical_height(height, viewport_width, content_width, scale);
    let work_height = f64::from(monitor.work.bottom - monitor.work.top) / scale;
    let max_height = (work_height * 0.86).clamp(1.0, 940.0);
    let min_height = 180.0_f64.min(max_height);
    let fitted_height = natural_height.clamp(min_height, max_height);
    let physical_height = tyl_core::locator::to_physical(fitted_height, scale).max(1);
    let current_height = i32::try_from(size.height).unwrap_or(i32::MAX);
    let current_width = i32::try_from(size.width).unwrap_or(i32::MAX);
    let margin = tyl_core::locator::to_physical(8.0, scale).max(1);

    let min_x = monitor.work.left + margin;
    let max_x = (monitor.work.right - current_width - margin).max(min_x);
    let min_y = monitor.work.top + margin;
    let max_y = (monitor.work.bottom - physical_height - margin).max(min_y);
    let preferred_y = if grow_upward {
        position.y + current_height - physical_height
    } else {
        position.y
    };
    let next_position = tauri::PhysicalPosition::new(
        position.x.clamp(min_x, max_x),
        preferred_y.clamp(min_y, max_y),
    );

    let _ = w.set_size(tauri::PhysicalSize::new(size.width, physical_height as u32));
    let _ = w.set_position(next_position);
    crate::logger::log_str(&format!(
        "[popup] 高度适配: css={height:.0} viewport={viewport_width:.0} physical_width={content_width} scale={scale} requested={natural_height:.0} applied={fitted_height:.0} grow_upward={grow_upward}"
    ));
}

fn css_to_logical_height(height: f64, viewport_width: f64, physical_width: u32, scale: f64) -> f64 {
    if viewport_width.is_finite() && viewport_width > 0.0 && physical_width > 0 {
        height * f64::from(physical_width) / viewport_width / scale
    } else {
        height
    }
}

#[cfg(test)]
mod layout_tests {
    use super::css_to_logical_height;

    #[test]
    fn accounts_for_webview_zoom_and_mixed_dpi() {
        assert_eq!(css_to_logical_height(400.0, 560.0, 840, 1.5), 400.0);
        assert_eq!(css_to_logical_height(400.0, 448.0, 840, 1.5), 500.0);
        assert_eq!(css_to_logical_height(400.0, 560.0, 1120, 2.0), 400.0);
        assert_eq!(css_to_logical_height(400.0, 0.0, 840, 1.5), 400.0);
    }
}

/// TTS 朗读（词典卡片喇叭按钮 / 译文朗读）。
#[tauri::command]
pub fn speak(text: String) {
    #[cfg(target_os = "windows")]
    crate::translate::tts::speak(&text);
    #[cfg(not(target_os = "windows"))]
    let _ = text;
}

/// Fetch the requested dialect through the configured HTTP client; the WebView plays it.
#[tauri::command]
pub async fn pronunciation_audio(word: String, accent: String) -> Result<Vec<u8>, String> {
    if !crate::translate::dictionary::is_single_word(&word) {
        return Err("请选择一个英文单词".into());
    }
    let voice_type = match accent.as_str() {
        "uk" => "1",
        "us" => "2",
        _ => return Err("未知发音类型".into()),
    };
    let response = crate::translate::client()
        .get("https://dict.youdao.com/dictvoice")
        .query(&[("audio", word.as_str()), ("type", voice_type)])
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .map_err(|_| "发音加载失败，请检查网络")?
        .error_for_status()
        .map_err(|_| "发音服务暂不可用")?;
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !content_type.starts_with("audio/") && !content_type.contains("octet-stream") {
        return Err("暂无此单词的发音".into());
    }
    let bytes = response.bytes().await.map_err(|_| "发音下载失败")?;
    if bytes.len() < 100 || bytes.len() > 2_000_000 {
        return Err("暂无此单词的发音".into());
    }
    Ok(bytes.to_vec())
}

fn popup_owner(app: &AppHandle) -> usize {
    #[cfg(target_os = "windows")]
    if let Some(window) = app.get_webview_window("popup") {
        return window.hwnd().map(|h| h.0 as usize).unwrap_or_default();
    }
    0
}

#[tauri::command]
pub async fn copy_translation(app: AppHandle, text: String) -> Result<(), String> {
    let owner = popup_owner(&app);
    tauri::async_runtime::spawn_blocking(move || crate::desktop_actions::copy(owner, &text))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn replace_selection(
    app: AppHandle,
    text: String,
    request_id: u64,
) -> Result<(), String> {
    let owner = popup_owner(&app);
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::desktop_actions::replace(owner, request_id, &text)
    })
    .await
    .map_err(|e| e.to_string())?;
    if let Err(error) = &result {
        crate::logger::warn(&format!("[replace] request={request_id} {error}"));
        if crate::desktop_actions::is_current(request_id) {
            if let Some(window) = app.get_webview_window("popup") {
                crate::window_ctl::show_focused(&window);
            }
        }
    }
    result
}

// ── 设置 ─────────────────────────────────────────────────

/// Tests the entered AI fields without saving them or sending selected text.
#[tauri::command]
pub async fn test_llm(config: crate::settings::LlmSettings) -> Result<String, String> {
    let config = crate::translate::llm::LlmConfig {
        base_url: config.base_url,
        api_key: config.api_key,
        model: config.model,
    };
    let started = std::time::Instant::now();
    let result =
        crate::translate::llm::translate_stream(&config, "Good morning.", "zh-CN", &mut |_| {})
            .await;
    match result {
        Ok(_) => Ok(format!(
            "连接成功，已收到译文（{:.1} 秒）",
            started.elapsed().as_secs_f64()
        )),
        Err(error) => {
            crate::logger::warn(&format!("[llm/test] {error}"));
            Err(error)
        }
    }
}

#[tauri::command]
pub fn get_settings() -> crate::settings::Settings {
    crate::settings::current()
}

#[tauri::command]
pub fn save_settings(settings: crate::settings::Settings) -> Result<(), String> {
    // 手动代理格式前置校验：错了当场报，不让用户带着坏代理静默直连。
    if settings.proxy.mode == "manual" {
        let url = settings.proxy.url.trim();
        if url.is_empty() {
            return Err("代理地址不能为空（或改用其他模式）".into());
        }
        let valid = url.starts_with("http://")
            || url.starts_with("https://")
            || url.starts_with("socks5://");
        if !valid {
            return Err("代理地址需以 http:// 或 socks5:// 开头".into());
        }
    }
    crate::settings::save(settings)?;
    // 代理变更 → 重建 HTTP client（下次请求生效新代理）
    crate::translate::reset_client();
    Ok(())
}

/// 开机自启状态。
#[tauri::command]
pub fn get_autostart(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// 设置开机自启。
#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    if enabled {
        app.autolaunch().enable().map_err(|e| e.to_string())?;
    } else {
        app.autolaunch().disable().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 前端加载完成的通知。
#[tauri::command]
pub fn frontend_ready(app: AppHandle, state: tauri::State<'_, crate::ReadyFlag>) {
    state.0.store(true, std::sync::atomic::Ordering::SeqCst);
    crate::logger::log_str("前端就绪 (frontend_ready)");
    if let Some(window) = app.get_webview_window("popup") {
        crate::webview_memory::hidden(&window);
    }
}

/// 前端 console 桥：webview 里的日志落到后端日志文件（生产无 devtools 时的眼睛）。
#[tauri::command]
pub fn frontend_log(message: String) {
    crate::logger::log_str(&format!("[前端] {message}"));
}
