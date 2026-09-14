//! MyMemory 免费翻译（api.mymemory.translated.net，国内直连，免 key）。
//!
//! 免费额度：匿名 5000 字/天 + 每 IP 约 1000 请求/日（实测足够个人
//! 划词使用）。langpair 支持源语言 Autodetect（服务端检测）。
//!
//! 已实测（2026-09，国内网络直连）：
//! - en|zh-CN / zh-CN|en 双向质量良好
//! - Autodetect 检测准确（en / zh-CN）
//! - 响应含 matches 人工译例（本工具只取 responseData.translatedText）

use serde_json::Value;

/// 翻译。源语言自动检测（langpair=Autodetect|<target>）。
/// 匿名接口单次最多 500 字符；保守按 450 UTF-16 units 自然分段。
pub async fn translate(text: &str, source: &str, target: &str) -> Result<String, String> {
    let parts = super::iciba::split_text(text, 450);
    let mut translated = Vec::with_capacity(parts.len());
    for part in &parts {
        if part.trim().is_empty() {
            translated.push(String::new());
        } else {
            translated.push(translate_part(part.trim(), source, target).await?);
        }
    }
    Ok(super::iciba::join_translations(&parts, &translated))
}

async fn translate_part(text: &str, source: &str, target: &str) -> Result<String, String> {
    let client = super::client();
    let source = if source == "auto" {
        "Autodetect"
    } else {
        source
    };
    let langpair = format!("{}|{}", map_target(source), map_target(target));
    let resp = client
        .get("https://api.mymemory.translated.net/get")
        .query(&[("q", text), ("langpair", langpair.as_str())])
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    let v: Value = resp.json().await.map_err(|e| format!("json: {e}"))?;

    let status = v
        .pointer("/responseStatus")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if status != 200 {
        let msg = v
            .pointer("/responseDetails")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Err(format!("mymemory {status}: {msg}"));
    }
    v.pointer("/responseData/translatedText")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "empty translation".to_string())
}

/// 目标语言映射：zh-CN 原生支持，无需映射；保留扩展点。
fn map_target(target: &str) -> &str {
    target
}

#[cfg(test)]
mod tests {
    #[test]
    fn long_queries_stay_below_service_limit_without_data_loss() {
        let input = "Every sentence must remain present. ".repeat(80) + "FINAL7391";
        let parts = super::super::iciba::split_text(&input, 450);
        assert_eq!(parts.concat(), input);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|part| part.encode_utf16().count() <= 450));
    }

    #[tokio::test]
    #[ignore = "Opt-in: sends fixed public fixtures to MyMemory"]
    async fn live_long_text_smoke() {
        let input = "Every sentence must remain present. ".repeat(35) + "FINAL7391";
        let translated = super::translate(&input, "auto", "zh-CN").await.unwrap();
        assert!(translated.contains("7391"));
    }
}
