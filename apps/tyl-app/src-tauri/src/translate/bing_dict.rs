//! Bing 词典（cn.bing.com tlookupv3，国内直连）。
//! 仅词典用——翻译已切微软官方端点（bing.rs 的 X-MT-Signature）。
//!
//! 会话凭据从 /translator 页面刮取（IG/IID/params_AbusePreventionHelper），
//! 约 10 分钟有效，205 时自动重刮。

use serde_json::Value;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TRANSLATOR_PAGE: &str = "https://cn.bing.com/translator";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

#[derive(Clone, Debug)]
struct Session {
    ig: String,
    iid: String,
    token: String,
    key: String,
}

static SESSION: Mutex<Option<(Session, Instant)>> = Mutex::new(None);
const SESSION_TTL: Duration = Duration::from_secs(8 * 60);

fn ua_headers(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    req.header("User-Agent", USER_AGENT)
        .header("Referer", TRANSLATOR_PAGE)
        .header("Origin", "https://cn.bing.com")
}

async fn fetch_session(client: &reqwest::Client) -> Result<Session, String> {
    let html = ua_headers(client.get(TRANSLATOR_PAGE))
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?
        .text()
        .await
        .map_err(|e| format!("read: {e}"))?;

    let ig = extract(&html, "IG:\"", 32).ok_or("IG not found")?;
    let iid = html
        .split("data-iid=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .map(str::to_string)
        .ok_or("IID not found")?;
    let abuse = html
        .split("params_AbusePreventionHelper = [")
        .nth(1)
        .and_then(|s| s.split(']').next())
        .ok_or("abuse params not found")?;
    let mut parts = abuse.split(',');
    let key = parts
        .next()
        .ok_or("key missing")?
        .trim_matches('"')
        .to_string();
    let token = parts
        .next()
        .ok_or("token missing")?
        .trim_matches('"')
        .to_string();

    if ig.len() < 16 || token.is_empty() {
        return Err("session parse failed".into());
    }
    Ok(Session {
        ig,
        iid,
        token,
        key,
    })
}

fn extract(html: &str, marker: &str, len: usize) -> Option<String> {
    html.split(marker).nth(1).map(|s| {
        s.chars()
            .take(len)
            .filter(|c| c.is_ascii_hexdigit())
            .collect()
    })
}

async fn session(client: &reqwest::Client) -> Result<Session, String> {
    // 快路径：缓存命中直接 clone（guard 不跨 await）
    {
        let guard = SESSION.lock().unwrap();
        if let Some((s, at)) = guard.as_ref() {
            if at.elapsed() < SESSION_TTL {
                return Ok(s.clone());
            }
        }
    }
    let s = fetch_session(client).await?;
    *SESSION.lock().unwrap() = Some((s.clone(), Instant::now()));
    Ok(s)
}

fn invalidate_session() {
    *SESSION.lock().unwrap() = None;
}

/// 词典查询（tlookupv3）。返回原始 JSON 由 dictionary.rs 解析。
pub async fn lookup_raw(word: &str) -> Result<Value, String> {
    let client = super::client();
    for attempt in 0..2 {
        let s = session(&client).await?;
        let resp = ua_headers(
            client
                .post(format!(
                    "https://cn.bing.com/tlookupv3?isVertical=1&IG={}&IID={}",
                    s.ig, s.iid
                ))
                .form(&[
                    ("from", "en"),
                    ("to", "zh-Hans"),
                    ("text", word),
                    ("token", &s.token),
                    ("key", &s.key),
                ]),
        )
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| format!("read: {e}"))?;
        if status.as_u16() == 200 {
            // 偶发响应前缀垃圾字符，截到首个 [
            if let Some(json_start) = body.find('[') {
                if let Ok(v) = serde_json::from_str::<Value>(&body[json_start..]) {
                    return Ok(v);
                }
            }
        }
        if attempt == 0 {
            invalidate_session();
            continue;
        }
        let snippet: String = body.chars().take(120).collect();
        return Err(format!("bing dict http {status}: {snippet}"));
    }
    Err("bing dict: session retry exhausted".into())
}
