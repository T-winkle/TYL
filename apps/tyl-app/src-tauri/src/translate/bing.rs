//! 微软翻译官方端点（api.cognitive.microsofttranslator.com，国内直连）。
//!
//! 鉴权用 X-MT-Signature（GTranslate 逆向的 MSTranslatorAndroidApp
//! HMAC-SHA256 签名——Android 客户端私钥是公开常量；STranslate 同款，
//! 2026-09 实测 ~350ms 稳定）。
//!
//! 相比旧 bing.rs（刮 cn.bing.com 页面拿 IG/IID/token + 205 重刮）：
//! 一次请求直达、无会话状态、无页面解析——更快更稳。Bing 的 tlookupv3
//! 词典端点保留（词典数据仍走它，见 dictionary.rs）。

use serde_json::Value;

const ENDPOINT: &str = "api.cognitive.microsofttranslator.com/translate?api-version=3.0";
/// MSTranslatorAndroidApp 私钥（公开常量，社区皆知）。
const PRIVATE_KEY: [u8; 64] = [
    0xa2, 0x29, 0x3a, 0x3d, 0xd0, 0xdd, 0x32, 0x73, 0x97, 0x7a, 0x64, 0xdb, 0xc2, 0xf3, 0x27, 0xf5,
    0xd7, 0xbf, 0x87, 0xd9, 0x45, 0x9d, 0xf0, 0x5a, 0x09, 0x66, 0xc6, 0x30, 0xc6, 0x6a, 0xaa, 0x84,
    0x9a, 0x41, 0xaa, 0x94, 0x3a, 0xa8, 0xd5, 0x1a, 0x6e, 0x4d, 0xaa, 0xc9, 0xa3, 0x70, 0x12, 0x35,
    0xc7, 0xeb, 0x12, 0xf6, 0xe8, 0x23, 0x07, 0x9e, 0x47, 0x10, 0x95, 0x91, 0x88, 0x55, 0xd8, 0x17,
];

/// 目标语言映射：内部码 → 微软码。
fn map_target(target: &str) -> String {
    match target {
        "zh-CN" => "zh-Hans".into(),
        "zh-TW" => "zh-Hant".into(),
        _ => target.into(),
    }
}

/// X-MT-Signature 请求头（不含协议头的 URL 参与 signing）。
fn mt_signature(path: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;
    let guid = uuid_simple();
    let escaped = url_encode(path);
    // RFC 1123 GMT，"%a, %d %b %Y %H:%M:%SGMT"
    let dt = http_date_now();

    let mut msg = format!("MSTranslatorAndroidApp{escaped}{dt}{guid}").to_lowercase();
    msg.make_ascii_lowercase();

    let mut mac = HmacSha256::new_from_slice(&PRIVATE_KEY).expect("hmac key");
    mac.update(msg.as_bytes());
    let b64 = base64_encode(&mac.finalize().into_bytes());
    format!("MSTranslatorAndroidApp::{b64}::{dt}::{guid}")
}

/// 简版 UUID v4（hex 无连字符）——不需要加密强度，随机即可。
fn uuid_simple() -> String {
    // 用进程地址熵 + 计数器 + 时间，避免引入 uuid crate
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{t:032x}{c:016x}")[..32].to_string()
}

/// percent-encode（大写 hex，.NET Uri.EscapeDataString 兼容）。
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// RFC 1123 日期（GMT），无外部 chrono：手工算星期。
fn http_date_now() -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days_total = now / 86400;
    let secs_today = now % 86400;
    // 1970-01-01 是周四
    let weekday = DAYS[((days_total + 4) % 7) as usize];
    // 公历换算（1970 起，2100 年前准确）
    let mut year = 1970u64;
    let mut days = days_total;
    loop {
        let leap = year.is_multiple_of(4) && !year.is_multiple_of(100) || year.is_multiple_of(400);
        let ylen = if leap { 366 } else { 365 };
        if days >= ylen {
            days -= ylen;
            year += 1;
        } else {
            break;
        }
    }
    let leap = year.is_multiple_of(4) && !year.is_multiple_of(100) || year.is_multiple_of(400);
    let mlens = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0usize;
    while days >= mlens[month] {
        days -= mlens[month];
        month += 1;
    }
    format!(
        "{weekday}, {:02} {} {year} {:02}:{:02}:{:02}GMT",
        days + 1,
        MONTHS[month],
        secs_today / 3600,
        (secs_today % 3600) / 60,
        secs_today % 60
    )
}

/// 标准 base64（无依赖实现，签名场景足够）。
fn base64_encode(data: &[u8]) -> String {
    const TBL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TBL[(n >> 18) as usize & 63] as char);
        out.push(TBL[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TBL[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TBL[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// 翻译。源语言省略（服务端 auto 检测，响应带 detectedLanguage）。
pub async fn translate(text: &str, source: &str, target: &str) -> Result<String, String> {
    let mut path = format!("{ENDPOINT}&to={}", map_target(target));
    if source != "auto" {
        path.push_str("&from=");
        path.push_str(&map_target(source));
    }
    let client = super::client();
    let resp = client
        .post(format!("https://{path}"))
        .header("X-MT-Signature", mt_signature(&path))
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/127.0.0.0 Safari/537.36")
        .json(&serde_json::json!([{ "Text": text }]))
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    let v: Value = resp.json().await.map_err(|e| format!("json: {e}"))?;
    v.pointer("/0/translations/0/text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "no translation".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_date_format() {
        let d = http_date_now();
        // "Thu, 04 Sep 2026 12:34:56GMT" 形态
        assert!(d.ends_with("GMT"), "{d}");
        assert_eq!(d.len(), 28, "{d}");
        assert!(d.starts_with(|c: char| c.is_ascii_uppercase()), "{d}");
    }

    #[test]
    fn urlencode_matches_dotnet() {
        assert_eq!(
            url_encode(
                "api.cognitive.microsofttranslator.com/translate?api-version=3.0&to=zh-Hans"
            ),
            "api.cognitive.microsofttranslator.com%2Ftranslate%3Fapi-version%3D3.0%26to%3Dzh-Hans"
        );
    }

    #[tokio::test]
    #[ignore = "Opt-in: sends fixed multilingual fixtures to Bing"]
    async fn live_fixed_language_pair_smoke() {
        for (text, source, target) in [
            ("Bonjour tout le monde.", "fr", "en"),
            ("Good morning.", "en", "ja"),
            ("繁體中文翻譯測試。", "zh-TW", "en"),
        ] {
            let translated = translate(text, source, target).await.unwrap();
            assert!(!translated.trim().is_empty(), "{source} -> {target}");
        }
    }

    #[test]
    fn base64_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}
