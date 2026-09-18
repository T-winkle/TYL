//! 热键 → 锚点 → 取词 → 定位 → 弹窗 → 翻译 的编排。
//!
//! 时序关键点：弹窗显示不等翻译（翻译流式补上）；取词在工作线程执行，
//! UI 线程只做窗口操作和事件推送。

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tyl_core::capture::CaptureOutcome;
use tyl_core::locator::{place_popup, Size};

use crate::translate;
use crate::window_ctl;

pub(crate) const TRANSLATE_EVENT: &str = "tyl://translate";

#[derive(Default)]
struct EngineRequests {
    request_id: u64,
    translation_revision: u64,
    engines: HashSet<String>,
}

impl EngineRequests {
    fn claim(&mut self, request_id: u64, translation_revision: u64, engine: &str) -> bool {
        if request_id < self.request_id
            || request_id == self.request_id && translation_revision < self.translation_revision
        {
            return false;
        }
        if request_id > self.request_id || translation_revision > self.translation_revision {
            self.request_id = request_id;
            self.translation_revision = translation_revision;
            self.engines.clear();
        }
        self.engines.insert(engine.into())
    }
}

// Eager translation, fallback and lazy tabs share ownership of each engine.
// Retain only one capture's IDs, never source text or translation results.
pub(crate) fn claim_engine(request_id: u64, translation_revision: u64, engine: &str) -> bool {
    static REQUESTS: std::sync::LazyLock<Mutex<EngineRequests>> =
        std::sync::LazyLock::new(|| Mutex::new(EngineRequests::default()));
    REQUESTS
        .lock()
        .unwrap()
        .claim(request_id, translation_revision, engine)
}

/// 推给前端的事件载荷（Popup.tsx 的 CapturedPayload 对应）。
#[derive(Serialize, Clone)]
pub struct CapturedEvent {
    /// 本次划词请求 ID。前端用它隔离并发请求，防止旧译文覆盖新选区。
    pub request_id: u64,
    pub text: String,
    pub source: SourceInfo,
    pub elapsed_ms: u64,
    pub target_exe: Option<String>,
    /// 本次参与翻译的引擎列表（主引擎在前）。
    pub engines: Vec<String>,
    /// "tabs"（按需切换）| "stacked"（全部展开）。
    pub result_display: String,
    pub source_language: String,
    pub detected_source_language: String,
    pub target_language: String,
    pub translation_revision: u64,
    /// 初始窗口位于选区上方；后续增高时保持底边，避免盖住原文。
    pub grow_upward: bool,
    pub theme: String,
    pub color_scheme: String,
    pub language: String,
    pub show_source: bool,
    pub is_word: bool,
    pub dictionary: bool,
    pub can_replace: bool,
    pub replace_requires_verification: bool,
    #[cfg(feature = "memory-bench")]
    pub benchmark: bool,
}

#[derive(Serialize, Clone)]
pub struct SourceInfo {
    pub kind: &'static str,
    pub restored: Option<bool>,
}

/// 翻译流事件（前端 tyl://translate）。
#[derive(Serialize, Clone)]
pub struct TranslateEvent {
    /// 对应 [`CapturedEvent::request_id`]。
    pub request_id: u64,
    /// "start" | "chunk" | "done" | "error" | "dict"
    pub phase: &'static str,
    pub text: String,
    /// 使用的服务（"bing" / "youdao" / "transmart" / "llm" / "dict"…）。
    pub service: String,
    pub translation_revision: u64,
}

/// 管线状态：持有平台取词器 + 防抖计数。
pub struct Pipeline {
    #[allow(clippy::struct_field_names)]
    capture: Option<crate::platform_capture::CaptureHandle>,
    /// 热键连打防抖：递增的代际号，过期的取词/翻译结果直接丢弃。
    generation: Arc<AtomicU64>,
    /// 平台取词单飞锁。翻译请求可以并行，但 UIA/剪贴板取词不能重叠。
    capture_in_flight: AtomicBool,
    /// 共享 tokio runtime（翻译请求）。
    rt: tokio::runtime::Handle,
}

impl Pipeline {
    pub fn new() -> Self {
        Self {
            capture: crate::platform_capture::spawn_capture(),
            generation: Arc::new(AtomicU64::new(0)),
            capture_in_flight: AtomicBool::new(false),
            rt: crate::runtime::handle(),
        }
    }

    /// 热键回调（global-shortcut 线程）——必须立即返回，工作丢给线程。
    pub fn on_hotkey(app: AppHandle, pipeline: Arc<Pipeline>) {
        if pipeline
            .capture_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            crate::logger::trace("取词仍在进行：已忽略重叠请求，避免剪贴板事务冲突");
            return;
        }

        std::thread::spawn(move || {
            struct CaptureFlightGuard<'a>(&'a AtomicBool);
            impl Drop for CaptureFlightGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::Release);
                }
            }
            let _flight = CaptureFlightGuard(&pipeline.capture_in_flight);
            let gen = pipeline.generation.fetch_add(1, Ordering::SeqCst) + 1;
            crate::desktop_actions::invalidate();
            if let Err(e) = run_capture_and_show(app, &pipeline, gen) {
                crate::logger::error(&format!("[tyl] pipeline error: {e}"));
            }
        });
    }
}

/// 取词完成即触发翻译或词典（不等弹窗显示——网络往返与渲染并行）。
/// 整句先请求主引擎，失败时按配置顺序自动降级；其他 tab 仍按需加载。
/// 单词走词典卡片，LLM 保持流式。代际号防抖：旧任务事件被丢弃。
fn spawn_translation(
    app: AppHandle,
    pipeline: &Pipeline,
    gen: u64,
    text: String,
    direction: translate::TranslationDirection,
) {
    let rt = pipeline.rt.clone();
    let cfg = crate::settings::current();
    let is_word = cfg.dictionary.enabled
        && direction.target == "zh-CN"
        && translate::dictionary::is_single_word(&text);
    let guard = GenerationGuard {
        generation: Arc::clone(&pipeline.generation),
        request_id: gen,
        translation_revision: 0,
        source_language: direction.source,
        target_language: direction.target,
    };

    let app_emit = app.clone();
    std::thread::spawn(move || {
        if guard.is_stale() {
            return;
        }

        if is_word {
            // 单词只请求词典；前端进入引擎视图或词典失败时按需请求译文。
            guard.emit(&app_emit, "start", String::new(), "dict");
            match rt.block_on(translate::dictionary::lookup(
                &text,
                &cfg.dictionary.provider,
            )) {
                Ok(entry) => {
                    guard.emit(
                        &app_emit,
                        "dict",
                        serde_json::to_string(&entry).unwrap_or_default(),
                        "dict",
                    );
                }
                Err(_) => {
                    crate::logger::warn("[dict] 查询失败，转为按需引擎翻译");
                    guard.emit(&app_emit, "error", "词典暂不可用".into(), "dict");
                }
            }
        } else {
            translate_for_mode(&app_emit, &rt, &text, &cfg, &guard);
        }
    });
}

/// 把所有翻译事件绑定到一次划词请求；请求过期后后端也停止继续推送。
#[derive(Clone)]
struct GenerationGuard {
    generation: Arc<AtomicU64>,
    request_id: u64,
    translation_revision: u64,
    source_language: String,
    target_language: String,
}

impl GenerationGuard {
    fn is_stale(&self) -> bool {
        self.generation.load(Ordering::SeqCst) != self.request_id
    }

    fn emit(&self, app: &AppHandle, phase: &'static str, text: String, service: &str) -> bool {
        self.emit_named(app, TRANSLATE_EVENT, phase, text, service)
    }

    fn emit_named(
        &self,
        app: &AppHandle,
        event: &str,
        phase: &'static str,
        text: String,
        service: &str,
    ) -> bool {
        if self.is_stale() {
            return false;
        }
        let _ = app.emit(
            event,
            TranslateEvent {
                request_id: self.request_id,
                phase,
                text,
                service: service.into(),
                translation_revision: self.translation_revision,
            },
        );
        true
    }
}

/// 按需翻译架构：
/// - settings.engines 是有序启用列表（第一个 = 主引擎）
/// - 取词后只发主引擎请求——最快出首屏
/// - 前端切换引擎 tab 时 invoke translate_one 懒加载该引擎
/// - CapturedEvent.engines = 该列表（前端 tab 顺序同此）
///
/// 每次翻译带代际号（gen），旧请求结果作废（防串台）。
fn primary_is_llm(cfg: &crate::settings::Settings) -> bool {
    cfg.engines.first().map(String::as_str) == Some(translate::google::ENGINE_LLM)
}

/// 按用户选择决定只请求主引擎，还是并行请求全部启用引擎。
fn translate_for_mode(
    app: &AppHandle,
    rt: &tokio::runtime::Handle,
    text: &str,
    cfg: &crate::settings::Settings,
    guard: &GenerationGuard,
) {
    if cfg.result_display == crate::settings::RESULT_DISPLAY_STACKED {
        translate_all(app, rt, text, cfg, guard);
    } else {
        translate_configured(app, rt, text, cfg, guard);
    }
}

/// 全部展开模式：每个启用引擎独立请求、独立完成，慢引擎不会阻塞快引擎。
fn translate_all(
    app: &AppHandle,
    rt: &tokio::runtime::Handle,
    text: &str,
    cfg: &crate::settings::Settings,
    guard: &GenerationGuard,
) {
    for engine in cfg.engines.clone() {
        if guard.is_stale() {
            return;
        }
        let app = app.clone();
        let rt = rt.clone();
        let text = text.to_string();
        let cfg = cfg.clone();
        let guard = guard.clone();
        std::thread::spawn(move || {
            if engine == translate::google::ENGINE_LLM {
                if cfg.llm.api_key.trim().is_empty() {
                    if !claim_engine(guard.request_id, guard.translation_revision, &engine) {
                        return;
                    }
                    guard.emit(&app, "start", String::new(), &engine);
                    guard.emit(&app, "error", "AI 引擎未配置 API Key".into(), &engine);
                } else {
                    translate_llm(&app, &rt, &text, &cfg, &guard, false);
                }
                return;
            }

            if !claim_engine(guard.request_id, guard.translation_revision, &engine) {
                return;
            }
            guard.emit(&app, "start", String::new(), &engine);
            match rt.block_on(run_engine(
                &engine,
                &text,
                &guard.source_language,
                &guard.target_language,
            )) {
                Ok(translated) if !translated.trim().is_empty() => {
                    guard.emit(&app, "done", translated, &engine);
                }
                Ok(_) => {
                    guard.emit(&app, "error", "empty".into(), &engine);
                }
                Err(e) => {
                    crate::logger::warn(&format!(
                        "[translate/all] {engine} 失败（省略服务原始响应）"
                    ));
                    guard.emit(&app, "error", e, &engine);
                }
            }
        });
    }
}

/// 执行当前配置的主引擎；LLM 未配置或请求失败时降级到传统引擎链。
fn translate_configured(
    app: &AppHandle,
    rt: &tokio::runtime::Handle,
    text: &str,
    cfg: &crate::settings::Settings,
    guard: &GenerationGuard,
) {
    if primary_is_llm(cfg) {
        if cfg.llm.api_key.trim().is_empty() {
            if !claim_engine(guard.request_id, guard.translation_revision, "llm") {
                return;
            }
            guard.emit(app, "start", String::new(), "llm");
            guard.emit(app, "error", "AI 引擎未配置 API Key".into(), "llm");
            translate_primary(app, rt, text, cfg, guard, Some("llm"));
        } else {
            translate_llm(app, rt, text, cfg, guard, true);
        }
    } else {
        translate_primary(app, rt, text, cfg, guard, None);
    }
}

/// 自动降级只走用户已启用的传统翻译引擎，并保持设置顺序。
fn fallback_engine_ids(cfg: &crate::settings::Settings, skip: Option<&str>) -> Vec<String> {
    cfg.engines
        .iter()
        .filter(|engine| engine.as_str() != translate::google::ENGINE_LLM)
        .filter(|engine| skip != Some(engine.as_str()))
        .cloned()
        .collect()
}

/// 单引擎执行（返回译文）。engine 是 service id（"bing"/"youdao"/…）。
pub async fn run_engine(
    engine: &str,
    text: &str,
    source: &str,
    target: &str,
) -> Result<String, String> {
    match engine {
        "bing" => translate::bing::translate(text, source, target).await,
        "youdao" => translate::youdao::translate(text, source, target).await,
        "transmart" => translate::transmart::translate(text, source, target).await,
        "yandex" => translate::yandex::translate(text, source, target).await,
        "iciba" => translate::iciba::translate(text, source, target).await,
        "mymemory" => translate::mymemory::translate(text, source, target).await,
        "google" => translate::google::translate(text, source, target).await,
        _ => Err(format!("unknown engine: {engine}")),
    }
}

/// 主引擎翻译 + emit（spawn_translation 的整句路径）。
fn translate_primary(
    app: &AppHandle,
    rt: &tokio::runtime::Handle,
    text: &str,
    cfg: &crate::settings::Settings,
    guard: &GenerationGuard,
    skip: Option<&str>,
) {
    for engine in fallback_engine_ids(cfg, skip) {
        if guard.is_stale() {
            return;
        }
        if !claim_engine(guard.request_id, guard.translation_revision, &engine) {
            continue;
        }
        guard.emit(app, "start", String::new(), &engine);
        match rt.block_on(run_engine(
            &engine,
            text,
            &guard.source_language,
            &guard.target_language,
        )) {
            Ok(t) if !t.trim().is_empty() => {
                guard.emit(app, "done", t, &engine);
                return;
            }
            Ok(_) => {
                crate::logger::warn(&format!("[translate] {engine} 失败: empty"));
                guard.emit(app, "error", "empty".into(), &engine);
            }
            Err(e) => {
                crate::logger::warn(&format!("[translate] {engine} 失败（省略服务原始响应）"));
                guard.emit(app, "error", e, &engine);
            }
        }
    }
}

/// LLM 流式翻译；网络/配置错误时降级默认链。
fn translate_llm(
    app: &AppHandle,
    rt: &tokio::runtime::Handle,
    text: &str,
    cfg: &crate::settings::Settings,
    guard: &GenerationGuard,
    fallback_on_error: bool,
) {
    let emit = |phase: &'static str, chunk: String| {
        guard.emit(app, phase, chunk, "llm");
    };
    if !claim_engine(guard.request_id, guard.translation_revision, "llm") {
        return;
    }
    emit("start", String::new());

    let llm_cfg = translate::llm::LlmConfig {
        base_url: cfg.llm.base_url.clone(),
        api_key: cfg.llm.api_key.clone(),
        model: cfg.llm.model.clone(),
    };
    let mut streamed_any = false;
    let mut on_chunk = |c: String| {
        streamed_any = true;
        emit("chunk", c);
    };

    match rt.block_on(translate::llm::translate_stream(
        &llm_cfg,
        text,
        &guard.source_language,
        &guard.target_language,
        &mut on_chunk,
    )) {
        Ok(full) if !full.trim().is_empty() => {
            if !streamed_any && !full.is_empty() {
                emit("chunk", full.clone());
            }
            emit("done", full);
        }
        Ok(_) => {
            emit("error", "empty".into());
            if fallback_on_error {
                translate_primary(app, rt, text, cfg, guard, Some("llm"));
            }
        }
        Err(e) => {
            crate::logger::warn(&format!("[translate] LLM 请求失败: {e}"));
            // 已经流了一半再降级会造成拼接错乱——只在零输出时降级。
            if !streamed_any {
                emit("error", e);
                if fallback_on_error {
                    translate_primary(app, rt, text, cfg, guard, Some("llm"));
                }
            } else {
                emit("error", "LLM 连接中断".into());
            }
        }
    }
}

fn run_capture_and_show(app: AppHandle, pipeline: &Pipeline, gen: u64) -> Result<(), String> {
    let cfg = crate::settings::current();
    // 1. 锚点（在取词前快照；DPI 感知在启动时已声明）
    let anchor = crate::platform_capture::capture_anchor();
    let target_window = crate::desktop_actions::foreground();
    crate::logger::log_str(&format!(
        "[gen {gen}] 锚点: exe={:?} cursor={:?}",
        anchor.target_exe, anchor.cursor
    ));

    // 2. 取词（平台实现；stub 平台返回 Err）
    let started = std::time::Instant::now();
    let outcome = pipeline
        .capture
        .as_ref()
        .ok_or("capture backend unavailable")?
        .capture_detailed(&anchor, Duration::from_millis(800));
    crate::logger::log_str(&format!(
        "[gen {gen}] 取词完成 {:?}ms: {}",
        started.elapsed().as_millis(),
        match &outcome {
            CaptureOutcome::Primary(_) => "primary(uia)".to_string(),
            CaptureOutcome::Fallback(_) => "fallback(clipboard)".to_string(),
            CaptureOutcome::Failed(e) => format!("failed: {e}"),
        }
    ));

    // 3. 过期检查：用户在取词期间又按了热键 → 这次结果作废
    if pipeline.generation.load(Ordering::SeqCst) != gen {
        crate::logger::log_str(&format!("[gen {gen}] 已过期，丢弃"));
        return Ok(());
    }

    let Some(window) = app.get_webview_window("popup") else {
        return Err("popup window not found".into());
    };

    match outcome {
        CaptureOutcome::Primary(t) | CaptureOutcome::Fallback(t) => {
            let text = tyl_core::text::normalize_selection(&t.text);
            if text.is_empty() {
                return Ok(());
            }
            let direction = translate::resolve_direction(&text, &cfg.language_routing);
            let use_dictionary = cfg.dictionary.enabled
                && direction.target == "zh-CN"
                && translate::dictionary::is_single_word(&text);
            let can_replace = crate::desktop_actions::can_offer_replacement(
                t.editable,
                anchor.target_exe.as_deref(),
            );
            crate::logger::log_str(&format!(
                "[replace] capture editable={:?} allowed={can_replace} identity={}",
                t.editable,
                t.selection_id.is_some()
            ));
            crate::desktop_actions::remember(
                gen,
                target_window,
                &t.text,
                can_replace,
                t.selection_id.clone(),
            );
            crate::logger::log_str(&format!(
                "[gen {gen}] 文本({} 字符)，不记录正文",
                t.text.chars().count()
            ));
            // 4. 定位：选区矩形优先，否则鼠标位置；clamp 到所在显示器工作区
            let (monitors, scale_factors_ok) = crate::platform_capture::monitors();
            if !scale_factors_ok || monitors.is_empty() {
                return Err("no monitors".into());
            }
            let anchor_point = t
                .selection_rect
                .map(|r| tyl_core::locator::Point {
                    x: r.left,
                    y: r.bottom,
                })
                .or(anchor.cursor)
                .unwrap_or(tyl_core::locator::Point { x: 200, y: 200 });
            let monitor = tyl_core::locator::pick_monitor(&monitors, anchor_point);
            // Initial size in CSS pixels; the frontend fits complete content after rendering.
            // locator/显示器矩形是物理像素 → 尺寸也要转物理（×scale），
            // 否则高 DPI 下窗口逻辑尺寸会变小，译文区被压没。
            let scale = monitor.scale;
            let popup_size = Size::new(
                (560.0 * scale).round() as i32,
                (260.0 * scale).round() as i32,
            );
            let margin = (8.0 * scale).round() as i32;
            let preferred_y = t
                .selection_rect
                .map(|rect| rect.bottom + margin)
                .or_else(|| anchor.cursor.map(|cursor| cursor.y + 2 * margin))
                .unwrap_or(monitor.work.top + margin);
            let grow_upward = preferred_y + popup_size.height > monitor.work.bottom - margin;
            let pos = place_popup(t.selection_rect, anchor.cursor, popup_size, monitor, margin);

            // 5. 移动窗口到位（此时仍隐藏，避免旧内容闪现）。
            let _ = window.set_position(tauri::PhysicalPosition::new(pos.x, pos.y));
            let _ = window.set_size(tauri::PhysicalSize::new(
                popup_size.width,
                popup_size.height,
            ));
            crate::logger::log_str(&format!(
                "[gen {gen}] 定位: pos=({},{}) 显示器 scale={}",
                pos.x, pos.y, monitor.scale
            ));

            // 6. 先推事件再显示窗口：前端渲染新内容（IPC 往返 ~10ms）
            crate::webview_memory::prepare_capture(&window)?;
            // 必须先于窗口可见完成，否则亮窗瞬间 WebView 里还是上一次
            // 的旧 DOM——连续取词时肉眼可见"上次文本残留闪一下"。
            let event = CapturedEvent {
                request_id: gen,
                text: text.clone(),
                source: match t.source {
                    tyl_core::CaptureSource::Uia => SourceInfo {
                        kind: "uia",
                        restored: None,
                    },
                    tyl_core::CaptureSource::Clipboard { restored } => SourceInfo {
                        kind: "clipboard",
                        restored: Some(restored),
                    },
                    tyl_core::CaptureSource::Manual => SourceInfo {
                        kind: "manual",
                        restored: None,
                    },
                },
                elapsed_ms: t.elapsed.as_millis() as u64,
                target_exe: anchor.target_exe,
                engines: cfg.engines.clone(),
                result_display: cfg.result_display.clone(),
                source_language: direction.source.clone(),
                detected_source_language: direction.detected_source.clone(),
                target_language: direction.target.clone(),
                translation_revision: 0,
                grow_upward,
                theme: cfg.theme.clone(),
                color_scheme: cfg.color_scheme.clone(),
                language: cfg.language.clone(),
                show_source: cfg.show_source,
                is_word: translate::dictionary::is_single_word(&text),
                dictionary: use_dictionary,
                can_replace,
                replace_requires_verification: can_replace && t.editable.is_none(),
                #[cfg(feature = "memory-bench")]
                benchmark: false,
            };
            let _ = app.emit("tyl://captured", event);

            // 6.5 翻译与窗口显示并行：网络往返（100ms+）不等渲染，
            // 弹窗亮起时译文已在路上（多引擎各自完成即推送）。
            spawn_translation(app.clone(), pipeline, gen, text, direction);

            // 给前端一帧的渲染时间（事件监听 + SolidJS 细粒度更新）。
            std::thread::sleep(Duration::from_millis(30));
            window_ctl::show_focused(&window);
            crate::logger::log_str(&format!("[gen {gen}] 事件已推送，窗口已显示"));
            Ok(())
        }
        CaptureOutcome::Failed(e) => {
            // 取词失败：不弹窗（M2 简化；M3 可弹 toast 提示原因）
            crate::logger::warn(&format!("[gen {gen}] 取词失败: {e}"));
            Ok(())
        }
    }
}

/// Benchmark input bypasses UIA only; real translation and popup rendering remain.
#[cfg(feature = "memory-bench")]
pub(crate) fn benchmark_present(
    app: &AppHandle,
    pipeline: &Pipeline,
    text: &str,
) -> Result<u64, String> {
    let window = app.get_webview_window("popup").ok_or("no popup")?;
    let gen = pipeline.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let cfg = crate::settings::current();
    let direction = translate::resolve_direction(text, &cfg.language_routing);
    let use_dictionary = cfg.dictionary.enabled
        && direction.target == "zh-CN"
        && translate::dictionary::is_single_word(text);
    let _ = window.set_size(tauri::LogicalSize::new(560.0, 260.0));
    let _ = window.center();
    crate::logger::log_str(&format!("[memory-bench] present id={gen}"));
    crate::webview_memory::prepare_capture(&window)?;
    app.emit(
        "tyl://captured",
        CapturedEvent {
            request_id: gen,
            text: text.into(),
            source: SourceInfo {
                kind: "manual",
                restored: None,
            },
            elapsed_ms: 0,
            target_exe: None,
            engines: cfg.engines,
            result_display: cfg.result_display,
            source_language: direction.source.clone(),
            detected_source_language: direction.detected_source.clone(),
            target_language: direction.target.clone(),
            translation_revision: 0,
            grow_upward: false,
            theme: cfg.theme,
            color_scheme: cfg.color_scheme,
            language: cfg.language,
            show_source: cfg.show_source,
            is_word: translate::dictionary::is_single_word(text),
            dictionary: use_dictionary,
            can_replace: false,
            replace_requires_verification: false,
            benchmark: true,
        },
    )
    .map_err(|e| e.to_string())?;
    spawn_translation(app.clone(), pipeline, gen, text.into(), direction);
    std::thread::sleep(Duration::from_millis(30));
    window_ctl::show_focused(&window);
    Ok(gen)
}

#[cfg(test)]
mod tests {
    #[test]
    fn eager_lazy_and_fallback_claim_an_engine_only_once_per_capture() {
        let mut requests = super::EngineRequests::default();
        assert!(requests.claim(10, 0, "bing"));
        assert!(!requests.claim(10, 0, "bing"));
        assert!(requests.claim(10, 0, "llm"));
        assert!(requests.claim(10, 1, "bing"));
        assert!(!requests.claim(10, 0, "youdao"));
        assert!(requests.claim(11, 0, "llm"));
        assert_eq!(requests.engines.len(), 1);
    }
    use super::*;

    #[test]
    fn fallback_engines_preserve_enabled_order_and_skip_llm() {
        let cfg = crate::settings::Settings {
            engines: vec![
                "llm".into(),
                "transmart".into(),
                "bing".into(),
                "youdao".into(),
            ],
            ..Default::default()
        };

        assert_eq!(
            fallback_engine_ids(&cfg, Some("bing")),
            vec!["transmart", "youdao"]
        );
    }

    #[test]
    fn generation_guard_marks_old_requests_stale() {
        let generation = Arc::new(AtomicU64::new(7));
        let guard = GenerationGuard {
            generation: Arc::clone(&generation),
            request_id: 7,
            translation_revision: 0,
            source_language: "auto".into(),
            target_language: "zh-CN".into(),
        };

        assert!(!guard.is_stale());
        generation.store(8, Ordering::SeqCst);
        assert!(guard.is_stale());
    }
}
