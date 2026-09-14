//! 腾讯交互式翻译 TranSmart（transmart.qq.com/api/imt，国内直连）。
//!
//! STranslate 同款参数（2026-09 实测）：
//! - fn=auto_translation_block + source.text_block（整块文本，非分行数组）
//! - header 只需 fn + client_key（固定串），Referer 用 yi.qq.com 域名
//! - auto 检测双向、技术文本质量好、响应 ~360ms

use serde_json::Value;

const ENDPOINT: &str = "https://transmart.qq.com/api/imt";
/// Web 前端形态的 client_key（UA-平台-uuid-时间戳，服务端不校验唯一性）。
const CLIENT_KEY: &str =
    "browser-chrome-110.0.0-Mac OS-df4bd4c5-a65d-44b2-a40f-42f34f3535f2-1677486696487";
const REFERER: &str = "https://yi.qq.com/zh-CN/index";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/110.0.0.0 Safari/537.36";

/// 目标语言映射：内部码 → TranSmart 码。
fn map_target(target: &str) -> String {
    match target {
        "zh-CN" => "zh".into(),
        _ => target.into(),
    }
}

/// 翻译。源语言 auto 检测（服务端）。
pub async fn translate(text: &str, source: &str, target: &str) -> Result<String, String> {
    let client = super::client();
    let resp = client
        .post(ENDPOINT)
        .header("Referer", REFERER)
        .header("User-Agent", USER_AGENT)
        .json(&serde_json::json!({
            "header": {
                "fn": "auto_translation_block",
                "client_key": CLIENT_KEY,
            },
            "type": "plain",
            "model_category": "normal",
            "source": {
                "lang": map_target(source),
                "text_block": text,
            },
            "target": {
                "lang": map_target(target),
            },
        }))
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    let v: Value = resp.json().await.map_err(|e| format!("json: {e}"))?;

    if v.pointer("/header/ret_code").and_then(Value::as_str) != Some("succ") {
        let msg = v
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| v.pointer("/header/ret_code").and_then(Value::as_str))
            .unwrap_or("unknown");
        return Err(format!("transmart: {msg}"));
    }
    // block 模式返回字符串（list 模式是数组——这里只处理 block）
    let out = v
        .get("auto_translation")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    if out.trim().is_empty() {
        return Err("empty translation".into());
    }
    Ok(out)
}
