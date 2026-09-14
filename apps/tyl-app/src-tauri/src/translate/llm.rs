//! OpenAI-compatible Chat Completions. Bounded, UTF-8-safe SSE and JSON parsing.
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;

const MAX_RESPONSE: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

/// Accept a base URL or full endpoint without rewriting provider-specific paths.
pub fn endpoint(base: &str) -> Result<String, String> {
    let mut url = reqwest::Url::parse(base.trim()).map_err(|_| "AI Base URL 格式无效")?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("AI Base URL 必须是 HTTP(S) 接口地址，不能包含账号密码".into());
    }
    let path = url.path().trim_end_matches('/');
    let path = if path.ends_with("/chat/completions") {
        path.to_string()
    } else {
        format!("{path}/chat/completions")
    };
    url.set_path(&path);
    url.set_fragment(None);
    Ok(url.to_string())
}

pub fn validate(cfg: &LlmConfig) -> Result<(), String> {
    if cfg.api_key.trim().is_empty() {
        return Err("AI 未配置 API Key".into());
    }
    if cfg.model.trim().is_empty() {
        return Err("AI 模型名称不能为空".into());
    }
    endpoint(&cfg.base_url)?;
    Ok(())
}

fn http_error(status: u16) -> String {
    let hint = match status {
        401 => "API Key 无效或已过期",
        403 => "无权访问，请检查模型权限或套餐接口",
        404 => "接口或模型不存在，请检查 Base URL 和模型名称",
        429 => "额度不足或请求过于频繁",
        400 | 422 => "请求参数不被支持，请检查模型名称和套餐接口",
        500..=599 => "服务暂时不可用，请稍后重试",
        _ => "请求失败，请检查服务配置",
    };
    // Never echo provider response bodies: they can contain the prompt/key.
    format!("AI HTTP {status}：{hint}")
}

fn response_text(value: &Value, streaming: bool) -> Result<Option<String>, String> {
    if value.get("error").is_some_and(|v| !v.is_null()) {
        let code = &value["error"]["code"];
        let code = code
            .as_str()
            .map(str::to_owned)
            .or_else(|| code.as_i64().map(|v| v.to_string()))
            .filter(|s| s.len() <= 32 && s.chars().all(|c| c.is_ascii_digit()));
        return Err(format!(
            "AI 服务拒绝请求{}，请检查额度、模型权限及接口地址",
            code.map(|c| format!("（代码 {c}）")).unwrap_or_default()
        ));
    }
    let field = if streaming { "delta" } else { "message" };
    let content = &value["choices"][0][field]["content"];
    Ok(content.as_str().map(str::to_owned).or_else(|| {
        content
            .as_array()
            .map(|parts| parts.iter().filter_map(|p| p["text"].as_str()).collect())
    }))
}

#[derive(Default)]
struct SseDecoder {
    pending: Vec<u8>,
    data: Vec<String>,
    data_bytes: usize,
    full: String,
    done: bool,
}

impl SseDecoder {
    fn dispatch(&mut self, on_chunk: &mut dyn FnMut(String)) -> Result<(), String> {
        if self.data.is_empty() {
            return Ok(());
        }
        let data = self.data.join("\n");
        self.data.clear();
        self.data_bytes = 0;
        if data.trim() == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let value: Value = serde_json::from_str(&data).map_err(|_| "AI 返回了无效的流式 JSON")?;
        if let Some(text) = response_text(&value, true)?.filter(|s| !s.is_empty()) {
            if self.full.len() + text.len() > MAX_RESPONSE {
                return Err("AI 返回内容过长".into());
            }
            self.full.push_str(&text);
            on_chunk(text);
        }
        Ok(())
    }

    fn line(&mut self, line: &[u8], on_chunk: &mut dyn FnMut(String)) -> Result<(), String> {
        let line = std::str::from_utf8(line)
            .map_err(|_| "AI 返回了无效的 UTF-8 文本")?
            .trim_end_matches('\r');
        if line.is_empty() {
            return self.dispatch(on_chunk);
        }
        // Wait for a complete byte line before UTF-8 decoding. SSE permits
        // `data:` with or without a single space and comments/keepalives.
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.strip_prefix(' ').unwrap_or(data);
            self.data_bytes += data.len();
            if self.data_bytes > MAX_RESPONSE {
                return Err("AI 流式事件过长".into());
            }
            self.data.push(data.into());
        }
        Ok(())
    }

    fn push(&mut self, bytes: &[u8], on_chunk: &mut dyn FnMut(String)) -> Result<(), String> {
        if self.done {
            return Ok(());
        }
        self.pending.extend_from_slice(bytes);
        let mut consumed = 0;
        while let Some(offset) = self.pending[consumed..].iter().position(|b| *b == b'\n') {
            let end = consumed + offset;
            let line = self.pending[consumed..end].to_vec();
            consumed = end + 1;
            self.line(&line, on_chunk)?;
            if self.done {
                break;
            }
        }
        self.pending.drain(..consumed);
        if self.pending.len() > MAX_RESPONSE {
            return Err("AI 流式事件过长".into());
        }
        Ok(())
    }

    fn finish(mut self, on_chunk: &mut dyn FnMut(String)) -> Result<String, String> {
        if !self.done {
            let rest = std::mem::take(&mut self.pending);
            if !rest.is_empty() {
                self.line(&rest, on_chunk)?;
            }
            self.dispatch(on_chunk)?;
        }
        nonempty(self.full)
    }
}

fn nonempty(text: String) -> Result<String, String> {
    if text.trim().is_empty() {
        Err("AI 未返回译文，请检查模型及接口是否支持 Chat Completions".into())
    } else {
        Ok(text)
    }
}

pub async fn translate_stream(
    cfg: &LlmConfig,
    text: &str,
    source: &str,
    target: &str,
    on_chunk: &mut (dyn FnMut(String) + Send),
) -> Result<String, String> {
    validate(cfg)?;
    translate_using(super::client(), cfg, text, source, target, on_chunk).await
}

async fn translate_using(
    client: reqwest::Client,
    cfg: &LlmConfig,
    text: &str,
    source: &str,
    target: &str,
    on_chunk: &mut (dyn FnMut(String) + Send),
) -> Result<String, String> {
    let target_name = language_name(target);
    let direction = if source == "auto" {
        format!("Translate the user's text to {target_name}.")
    } else {
        format!(
            "Translate the user's text from {} to {target_name}.",
            language_name(source)
        )
    };
    // Reasoning models may reject optional sampling parameters (temperature).
    let body = json!({
        "model": cfg.model.trim(), "stream": true,
        "messages": [
            {"role": "system", "content": format!("You are a translation engine. {direction} Output ONLY the translation, no explanations, no quotes.")},
            {"role": "user", "content": text}
        ]
    });
    let started = std::time::Instant::now();
    let response = client
        .post(endpoint(&cfg.base_url)?)
        .timeout(Duration::from_secs(120))
        .bearer_auth(cfg.api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "AI 请求超时，请稍后重试"
            } else {
                "AI 网络连接失败，请检查地址、代理与证书"
            }
        })?;
    if !response.status().is_success() {
        return Err(http_error(response.status().as_u16()));
    }
    let is_json = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/json"));
    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut json_bytes = Vec::new();
    let mut received = 0;
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(60), stream.next())
        .await
        .map_err(|_| "AI 等待响应超时，请检查模型或稍后重试")?
    {
        let bytes = chunk.map_err(|_| "AI 连接中断，请重试")?;
        received += bytes.len();
        if received > MAX_RESPONSE {
            return Err("AI 返回内容过长".into());
        }
        if is_json {
            json_bytes.extend_from_slice(&bytes);
        } else {
            decoder.push(&bytes, on_chunk)?;
            if decoder.done {
                break;
            }
        }
    }
    let full = if is_json {
        let value: Value =
            serde_json::from_slice(&json_bytes).map_err(|_| "AI 返回了无效的 JSON")?;
        let full = nonempty(response_text(&value, false)?.unwrap_or_default())?;
        on_chunk(full.clone());
        full
    } else {
        decoder.finish(on_chunk)?
    };
    crate::logger::info(&format!(
        "[llm] 请求完成 elapsed_ms={} chars={}",
        started.elapsed().as_millis(),
        full.chars().count()
    ));
    Ok(full)
}

fn language_name(language: &str) -> &'static str {
    match language {
        "zh-CN" => "Simplified Chinese",
        "zh-TW" => "Traditional Chinese",
        "en" => "English",
        "ja" => "Japanese",
        "ko" => "Korean",
        "fr" => "French",
        "de" => "German",
        "es" => "Spanish",
        "ru" => "Russian",
        "pt" => "Portuguese",
        _ => "the requested language",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn local_response(
        status: &str,
        mime: &str,
        body: &str,
    ) -> (LlmConfig, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client_may_close_after_headers = !status.starts_with('2');
        let response = format!("HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0; 1024];
                let read = socket.read(&mut chunk).unwrap();
                assert!(read > 0);
                request.extend_from_slice(&chunk[..read]);
                let request_text = String::from_utf8_lossy(&request);
                if let Some(end) = request_text.find("\r\n\r\n") {
                    let length: usize = request_text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|n| n.trim().parse().unwrap())
                        })
                        .unwrap();
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("POST /v1/chat/completions "));
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer fixture-key"));
            assert!(request.contains("\"stream\":true"));
            assert!(!request.contains("temperature"));
            for bytes in response.as_bytes().chunks(11) {
                if let Err(error) = socket.write_all(bytes) {
                    let expected_early_close = client_may_close_after_headers
                        && matches!(
                            error.kind(),
                            std::io::ErrorKind::BrokenPipe
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionAborted
                        );
                    assert!(
                        expected_early_close,
                        "fixture response write failed: {error}"
                    );
                    break;
                }
            }
        });
        (
            LlmConfig {
                base_url: format!("http://{address}/v1"),
                api_key: "fixture-key".into(),
                model: "fixture-model".into(),
            },
            thread,
        )
    }

    #[test]
    fn http_stream_json_and_failure_paths() {
        super::super::ensure_tls();
        for (status, mime, body, expected) in [
            ("200 OK", "text/event-stream", "data:{\"choices\":[{\"delta\":{\"content\":\"早上好\"}}]}\r\n\r\ndata:[DONE]\r\n\r\n", Some("早上好")),
            ("200 OK", "application/json", "{\"choices\":[{\"message\":{\"content\":\"早上好\"}}]}", Some("早上好")),
            ("401 Unauthorized", "application/json", "{\"error\":{\"message\":\"secret-prompt\"}}", None),
        ] {
            let (cfg, server) = local_response(status, mime, body);
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            let mut chunks = String::new();
            let result = crate::runtime::handle().block_on(translate_using(client, &cfg, "Good morning.", "en", "zh-CN", &mut |text| chunks.push_str(&text)));
            server.join().unwrap();
            match expected {
                Some(expected) => { assert_eq!(result.unwrap(), expected); assert_eq!(chunks, expected); },
                None => { let error = result.unwrap_err(); assert!(error.contains("401")); assert!(!error.contains("secret-prompt")); }
            }
        }
    }
    #[test]
    #[ignore = "Opt-in: sends a fixed short phrase to the configured provider"]
    fn configured_provider_smoke() {
        let path =
            std::env::var("TYL_LLM_TEST_SETTINGS").expect("explicit test settings path required");
        let input = std::fs::read_to_string(path).expect("read test config");
        let settings: crate::settings::Settings =
            serde_json::from_str(&input).expect("valid settings");
        let cfg = LlmConfig {
            base_url: settings.llm.base_url,
            api_key: settings.llm.api_key,
            model: settings.llm.model,
        };
        let mut chunks = 0;
        let result = crate::runtime::handle().block_on(translate_stream(
            &cfg,
            "Good morning.",
            "en",
            "zh-CN",
            &mut |_| {
                chunks += 1;
            },
        ));
        match result {
            Ok(text) => println!(
                "provider smoke: OK, chunks={chunks}, chars={}",
                text.chars().count()
            ),
            Err(error) => panic!("provider smoke: {error}"),
        }
    }
    #[test]
    fn endpoint_normalization() {
        assert_eq!(
            endpoint(" https://open.bigmodel.cn/api/v1/ ").unwrap(),
            "https://open.bigmodel.cn/api/v1/chat/completions"
        );
        for url in [
            "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions",
            "https://example.com/v1/chat/completions",
        ] {
            assert_eq!(endpoint(url).unwrap(), url);
        }
        assert_eq!(
            endpoint("https://example.com/custom/v2").unwrap(),
            "https://example.com/custom/v2/chat/completions"
        );
        assert!(endpoint("file:///test").is_err());
        assert!(endpoint("https://user:secret@example.com/v1").is_err());
    }
    #[test]
    fn sse_handles_every_utf8_split_crlf_and_no_space() {
        let wire = ":keepalive\r\ndata:{\"choices\":[{\"delta\":{\"content\":\"你好🌍\"}}]}\r\n\r\ndata: [DONE]\r\n\r\n";
        for split in 0..=wire.len() {
            let mut decoder = SseDecoder::default();
            let mut chunks = String::new();
            let mut emit = |text: String| chunks.push_str(&text);
            decoder.push(&wire.as_bytes()[..split], &mut emit).unwrap();
            decoder.push(&wire.as_bytes()[split..], &mut emit).unwrap();
            assert_eq!(decoder.finish(&mut emit).unwrap(), "你好🌍");
            assert_eq!(chunks, "你好🌍");
        }
    }
    #[test]
    fn sse_flushes_last_event_without_blank_line() {
        let mut decoder = SseDecoder::default();
        decoder
            .push(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}",
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(decoder.finish(&mut |_| {}).unwrap(), "ok");
    }
    #[test]
    fn json_fallback_and_safe_errors() {
        assert_eq!(
            response_text(&json!({"choices":[{"message":{"content":"译文"}}]}), false)
                .unwrap()
                .unwrap(),
            "译文"
        );
        let err = response_text(
            &json!({"error":{"code":1302,"message":"secret prompt or key"}}),
            true,
        )
        .unwrap_err();
        assert!(err.contains("1302"));
        assert!(!err.contains("secret"));
        assert!(http_error(401).contains("API Key"));
        assert!(http_error(404).contains("Base URL"));
        assert!(nonempty(String::new()).is_err());
    }
}
