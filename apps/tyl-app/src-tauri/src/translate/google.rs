//! Google 翻译官方 gtx 端点（需代理或可达网络）。
//! 国内直连不通；作为代理环境下的可选引擎。

use serde_json::Value;

/// 引擎 ID（settings.engine 取值）。
pub const ENGINE_LLM: &str = "llm";

/// 官方端点（唯一；旧 .com.hk 镜像实测不可用，已删除）。
fn endpoint() -> &'static str {
    "https://translate.googleapis.com/translate_a/single"
}

/// 一次性翻译。源语言 auto 检测。
pub async fn translate(text: &str, source: &str, target: &str) -> Result<String, String> {
    let client = super::client();
    let resp = client
        .get(endpoint())
        .query(&[
            ("client", "gtx"),
            ("sl", source),
            ("tl", target),
            ("dt", "t"),
            ("q", text),
        ])
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    let v: Value = resp.json().await.map_err(|e| format!("json: {e}"))?;

    let mut out = String::new();
    if let Some(segs) = v[0].as_array() {
        for seg in segs {
            if let Some(t) = seg[0].as_str() {
                out.push_str(t);
            }
        }
    }
    if out.is_empty() {
        return Err("empty translation".into());
    }
    Ok(out)
}
