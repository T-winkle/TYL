//! Yandex mobile-client translation endpoint (no user key required).

use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ENDPOINT: &str = "https://translate.yandex.net/api/v1/tr.json/translate";
const USER_AGENT: &str = "ru.yandex.translate/3.20.2024";
static UCID: LazyLock<Mutex<Option<(String, Instant)>>> = LazyLock::new(|| Mutex::new(None));
static NONCE: AtomicU64 = AtomicU64::new(0);

pub async fn translate(text: &str, source: &str, target: &str) -> Result<String, String> {
    // A target-only direction enables provider-side source detection.
    let direction = if source == "auto" && contains_han(text) {
        format!("zh-{}", map_language(target))
    } else if source == "auto" {
        map_language(target).to_string()
    } else {
        format!("{}-{}", map_language(source), map_language(target))
    };
    let parts = super::iciba::split_text(text, 7_000);
    let mut translated = Vec::with_capacity(parts.len());
    for part in &parts {
        if part.trim().is_empty() {
            translated.push(String::new());
        } else {
            translated.push(request_with_retry(part.trim(), &direction).await?);
        }
    }
    Ok(super::iciba::join_translations(&parts, &translated))
}

async fn request_with_retry(text: &str, direction: &str) -> Result<String, String> {
    let mut last_error = String::new();
    for attempt in 0..2 {
        match request(text, direction).await {
            Ok(result) => return Ok(result),
            Err(error) => last_error = error,
        }
        if attempt == 0 {
            tokio::time::sleep(Duration::from_millis(140)).await;
        }
    }
    Err(last_error)
}

async fn request(text: &str, direction: &str) -> Result<String, String> {
    let response = super::client()
        .post(ENDPOINT)
        .query(&[
            ("ucid", current_ucid()),
            ("srv", "android".into()),
            ("format", "text".into()),
        ])
        .header("User-Agent", USER_AGENT)
        .form(&[("text", text), ("lang", direction)])
        .send()
        .await
        .map_err(|error| format!("network: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("http {}", response.status()));
    }
    let value: Value = response
        .json()
        .await
        .map_err(|error| format!("json: {error}"))?;
    let output = value
        .pointer("/text/0")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if output.is_empty() {
        let code = value
            .get("code")
            .map(Value::to_string)
            .unwrap_or_else(|| "unknown".into());
        Err(format!("Yandex 未返回译文（{code}）"))
    } else {
        Ok(output.into())
    }
}

fn current_ucid() -> String {
    let mut cached = UCID.lock().unwrap();
    if let Some((value, created)) = cached.as_ref() {
        if created.elapsed() < Duration::from_secs(360) {
            return value.clone();
        }
    }
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let nonce = u128::from(NONCE.fetch_add(1, Ordering::Relaxed));
    let value = format!("{:032x}", time ^ nonce ^ u128::from(std::process::id()));
    *cached = Some((value.clone(), Instant::now()));
    value
}

fn map_language(language: &str) -> &str {
    match language {
        "zh-CN" | "zh-TW" => "zh",
        other => other,
    }
}

fn contains_han(text: &str) -> bool {
    text.chars().any(|ch| {
        matches!(ch, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}')
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn ucid_is_cached_and_has_guid_shape() {
        let first = super::current_ucid();
        assert_eq!(first, super::current_ucid());
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[tokio::test]
    #[ignore = "Opt-in: sends fixed public fixtures to Yandex"]
    async fn live_auto_direction_and_long_text_smoke() {
        let short = super::translate("This is a translation test.", "auto", "zh-CN")
            .await
            .unwrap();
        assert!(!short.is_empty());
        let reverse = super::translate("这是一次翻译测试。", "auto", "en")
            .await
            .unwrap();
        assert!(reverse.to_ascii_lowercase().contains("translation"));
        let long = "Every paragraph must remain complete. ".repeat(240) + "Final marker 7391.";
        let translated = super::translate(&long, "auto", "zh-CN").await.unwrap();
        assert!(translated.contains("7391"));
    }
}
