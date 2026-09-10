//! Deterministic renderer QA, compiled only with `memory-bench`.
//! PNGs are WebView2 CapturePreview outputs, not desktop/compositor screenshots.
use std::time::Duration;
use tauri::{Emitter, Manager, WebviewWindow};

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

#[cfg(target_os = "windows")]
fn capture(window: &WebviewWindow, name: &str) -> Result<(), String> {
    use webview2_com::{
        CapturePreviewCompletedHandler,
        Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
    };
    use windows::Win32::{
        Foundation::HGLOBAL,
        System::Com::{
            StructuredStorage::CreateStreamOnHGlobal, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET,
        },
    };
    let path = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name(format!("{}-{name}.png", crate::memory_bench::mode()));
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    window
        .with_webview(move |native| {
            let setup = (|| -> windows::core::Result<()> {
                let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true)? };
                let output = stream.clone();
                let completed = tx.clone();
                let callback = CapturePreviewCompletedHandler::create(Box::new(move |result| {
                    let outcome = (|| -> Result<(), String> {
                        result.map_err(|e| e.to_string())?;
                        let mut stat = STATSTG::default();
                        unsafe {
                            output
                                .Stat(&mut stat, STATFLAG_NONAME)
                                .map_err(|e| e.to_string())?;
                        }
                        if stat.cbSize > 16 * 1024 * 1024 {
                            return Err("capture exceeds 16 MiB".into());
                        }
                        let mut bytes = vec![0_u8; stat.cbSize as usize];
                        let mut read = 0;
                        unsafe {
                            output
                                .Seek(0, STREAM_SEEK_SET, None)
                                .map_err(|e| e.to_string())?;
                            output
                                .Read(
                                    bytes.as_mut_ptr().cast(),
                                    bytes.len() as u32,
                                    Some(&mut read),
                                )
                                .ok()
                                .map_err(|e| e.to_string())?;
                        }
                        if read as usize != bytes.len() {
                            return Err("incomplete capture".into());
                        }
                        std::fs::write(&path, bytes).map_err(|e| e.to_string())
                    })();
                    let _ = completed.send(outcome);
                    Ok(())
                }));
                unsafe {
                    native.controller().CoreWebView2()?.CapturePreview(
                        COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                        &stream,
                        &callback,
                    )?;
                }
                Ok(())
            })();
            if let Err(error) = setup {
                let _ = tx.send(Err(error.to_string()));
            }
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(8))
        .map_err(|e| e.to_string())?
}

#[cfg(not(target_os = "windows"))]
fn capture(_: &WebviewWindow, _: &str) -> Result<(), String> {
    Err("Windows-only renderer QA".into())
}

pub fn run(app: &tauri::AppHandle) -> Result<(), String> {
    use crate::pipeline::{CapturedEvent, SourceInfo, TranslateEvent};
    let window = app.get_webview_window("popup").ok_or("no popup")?;
    for (index, (theme, word)) in [
        ("light", false),
        ("light", true),
        ("dark", false),
        ("dark", true),
    ]
    .into_iter()
    .enumerate()
    {
        let scene = format!("{theme}-{}", if word { "dictionary" } else { "longtext" });
        let request_id = 1_000_000 + index as u64;
        crate::webview_memory::prepare_capture(&window)?;
        app.emit("tyl://captured", CapturedEvent {
            request_id, text: if word { "elegant".into() } else { "A thoughtfully designed tool makes reading easier. Clear hierarchy, comfortable spacing and smooth feedback create a reliable reading experience.".into() },
            source: SourceInfo {kind: "manual", restored: None}, elapsed_ms: 0, target_exe: None,
            engines: vec!["bing".into()], result_display: "tabs".into(), grow_upward: false,
            theme: theme.into(), color_scheme: "indigo".into(), show_source: false,
            is_word: word, dictionary: true, can_replace: false, replace_requires_verification: false, benchmark: true,
        }).map_err(|e| e.to_string())?;
        let text = if word {
            serde_json::json!({"word":"elegant","phonetic_uk":"ˈelɪɡənt","phonetic_us":"ˈelɪɡənt",
                "senses":[{"pos":"adj.","definitions":["优雅的；精致的；简洁巧妙的"]},
                {"pos":"例句","definitions":["an elegant solution · 一个简洁巧妙的解决方案"]}],
                "exam_types":["CET4","CET6","IELTS"]})
            .to_string()
        } else {
            "精心设计的工具，应当帮助人们更轻松地阅读。\n\n清晰的层次、舒适的留白，以及自然流畅的操作反馈，共同构成可靠的阅读体验。\n\n".repeat(14)
        };
        app.emit(
            crate::pipeline::TRANSLATE_EVENT,
            TranslateEvent {
                request_id,
                phase: if word { "dict" } else { "done" },
                text,
                service: if word { "dict" } else { "bing" }.into(),
            },
        )
        .map_err(|e| e.to_string())?;
        window
            .set_size(tauri::LogicalSize::new(560., 460.))
            .map_err(|e| e.to_string())?;
        window.center().map_err(|e| e.to_string())?;
        pause(30);
        crate::window_ctl::show_focused(&window);
        pause(1200);
        capture(&window, &scene)?;
        window
            .eval(format!(
                "window.__tylVisualScene={};{}",
                serde_json::to_string(&scene).unwrap(),
                include_str!("visual_motion.js")
            ))
            .map_err(|e| e.to_string())?;
        pause(9400);
        capture(&window, &format!("{scene}-after"))?;
        crate::window_ctl::hide(&window);
    }
    crate::open_settings_window(app);
    pause(2000);
    let settings = app.get_webview_window("settings").ok_or("no settings")?;
    for theme in ["light", "dark"] {
        settings.eval(format!("document.documentElement.dataset.theme='{theme}';document.documentElement.style.colorScheme='{theme}';document.querySelector('.content-scroll').scrollTop=10000;")).map_err(|e| e.to_string())?;
        pause(1000);
        capture(&settings, &format!("settings-{theme}"))?;
    }
    crate::logger::log_str("[visual-bench] settings ready for desktop inspection (30s)");
    pause(30000);
    settings.close().map_err(|e| e.to_string())?;
    Ok(())
}
