//! 有道翻译（dict.youdao.com/jsonapi_s，国内直连，web 端接口）。
//!
//! 签名（从 fanyi.youdao.com 前端 app.js 逆向，2026-09 验证）：
//!   sign = md5("webmain" + q + t + SECRET + md5(q + "webfanyi.webmain"))
//!   t    = 时间戳ms + len(q+"webfanyi.webmain")%10
//!   SECRET = "t2he2k4m2g6QKRigK0KAmSpXKgAezywG"（web 前端公开常量）
//!
//! 数据质量：翻译 auto 检测双向；词典含 us/uk 音标、词性释义、
//! 单词变形（三单/过去式）、考试标签（CET4/6/考研）。老接口
//! fanyi.youdao.com/translate_o（fanyidesk2 签名）已失效（errorCode 30）。

use futures_util::{stream, StreamExt, TryStreamExt};
use serde_json::Value;

const ENDPOINT: &str = "https://dict.youdao.com/jsonapi_s?doctype=json&jsonversion=4";
const SECRET: &str = "t2he2k4m2g6QKRigK0KAmSpXKgAezywG";
const KEYFROM: &str = "webfanyi.webmain";
const CLIENT: &str = "webmain";

pub(crate) fn md5_hex(s: &str) -> String {
    // 纯 Rust MD5（无依赖）：翻译工具不需要加密级性能，直接实现
    md5(s.as_bytes())
}

/// MD5（RFC 1321，紧凑实现——避免引入 md-5 crate 仅为这一个签名）。
fn md5(msg: &[u8]) -> String {
    let mut a: u32 = 0x6745_2301;
    let mut b: u32 = 0xefcd_ab89;
    let mut c: u32 = 0x98ba_dcfe;
    let mut d: u32 = 0x1032_5476;

    let s: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: [u32; 64] = std::array::from_fn(|i| {
        let v = (i as f64 + 1.0).sin().abs() * 4_294_967_296.0;
        v as u32
    });

    // padding
    let mut data = msg.to_vec();
    let bit_len = (msg.len() as u64).wrapping_mul(8);
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in data.as_chunks::<64>().0 {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        let (mut aa, mut bb, mut cc, mut dd) = (a, b, c, d);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((bb & cc) | (!bb & dd), i),
                1 => ((dd & bb) | (!dd & cc), (5 * i + 1) % 16),
                2 => (bb ^ cc ^ dd, (3 * i + 5) % 16),
                _ => (cc ^ (bb | !dd), (7 * i) % 16),
            };
            // F = f + A + K[i] + M[g]（A 参与求和——漏掉它是常见笔误）
            let f_sum = f
                .wrapping_add(aa)
                .wrapping_add(k[i])
                .wrapping_add(m[g])
                .rotate_left(s[i]);
            aa = dd;
            dd = cc;
            cc = bb;
            bb = bb.wrapping_add(f_sum);
        }
        a = a.wrapping_add(aa);
        b = b.wrapping_add(bb);
        c = c.wrapping_add(cc);
        d = d.wrapping_add(dd);
    }
    // MD5 标准输出 = 四个 32 位字的小端字节序列（每个字字节序反转）
    format!(
        "{:08x}{:08x}{:08x}{:08x}",
        a.swap_bytes(),
        b.swap_bytes(),
        c.swap_bytes(),
        d.swap_bytes()
    )
}

/// 目标语言映射：内部码 → 有道码。
fn map_target(target: &str) -> String {
    match target {
        "zh-CN" => "zh-CHS".into(),
        "zh-TW" => "zh-CHT".into(),
        _ => target.into(),
    }
}

fn sign(q: &str) -> (String, String) {
    let t = format!(
        "{}{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default(),
        format!("{q}{KEYFROM}").len() % 10
    );
    let inner = md5_hex(&format!("{q}{KEYFROM}"));
    let s = md5_hex(&format!("{CLIENT}{q}{t}{SECRET}{inner}"));
    (s, t)
}

/// jsonapi_s silently truncates long queries (observed around 600 units).
/// Stay below that limit, preserve every source byte and keep request order.
pub async fn translate(text: &str, target: &str) -> Result<String, String> {
    let parts = split_query(text, 400);
    let translated: Vec<String> = stream::iter(parts.iter().copied())
        .map(|part| async move {
            if part.trim().is_empty() {
                return Ok(String::new());
            }
            translate_part(part.trim(), target).await
        })
        .buffered(3)
        .try_collect()
        .await?;
    Ok(join_translations(&parts, &translated))
}

fn split_query(mut text: &str, limit: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    while !text.is_empty() {
        let mut units = 0;
        let mut end = 0;
        let mut sentence = 0;
        let mut whitespace = 0;
        for (index, ch) in text.char_indices() {
            if units + ch.len_utf16() > limit {
                break;
            }
            units += ch.len_utf16();
            end = index + ch.len_utf8();
            if ch.is_whitespace() {
                whitespace = end;
            }
            if ".!?。！？\n".contains(ch) {
                sentence = end;
            }
        }
        if end < text.len() {
            // Prefer a natural sentence/paragraph boundary; fall back to a
            // word boundary, then a Unicode-safe cut for unbroken text.
            end = if sentence > 0 {
                sentence
            } else if whitespace > 0 {
                whitespace
            } else {
                end
            };
        }
        parts.push(&text[..end]);
        text = &text[end..];
    }
    parts
}

fn join_translations(parts: &[&str], translated: &[String]) -> String {
    let mut output = String::new();
    for (index, result) in translated.iter().enumerate() {
        if index > 0 && !output.is_empty() && !result.is_empty() {
            let previous = parts[index - 1];
            let gap = format!(
                "{}{}",
                &previous[previous.trim_end().len()..],
                &parts[index][..parts[index].len() - parts[index].trim_start().len()]
            );
            let newlines = gap.chars().filter(|&ch| ch == '\n').count();
            if newlines > 0 {
                output.extend(std::iter::repeat_n('\n', newlines));
            } else if !(output.ends_with(char::is_whitespace)
                || output.chars().last().is_some_and(is_cjk)
                    && result.chars().next().is_some_and(is_cjk))
            {
                output.push(' ');
            }
        }
        output.push_str(result.trim());
    }
    output
}

fn is_cjk(ch: char) -> bool {
    matches!(ch, '\u{2e80}'..='\u{9fff}' | '\u{f900}'..='\u{faff}' | '\u{ff00}'..='\u{ffef}')
}

async fn translate_part(text: &str, target: &str) -> Result<String, String> {
    let (sgn, t) = sign(text);
    let client = super::client();
    let resp = client
        .post(ENDPOINT)
        .form(&[
            ("q", text),
            ("from", "auto"),
            ("to", map_target(target).as_str()),
            ("dict", "true"),
            ("t", t.as_str()),
            ("client", CLIENT),
            ("sign", sgn.as_str()),
            ("keyfrom", KEYFROM),
        ])
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    let v: Value = resp.json().await.map_err(|e| format!("json: {e}"))?;
    parse_part(&v, text)
}

fn parse_part(v: &Value, text: &str) -> Result<String, String> {
    if let Some(input) = v.pointer("/fanyi/input").and_then(Value::as_str) {
        if input.replace("\r\n", "\n").trim() != text.replace("\r\n", "\n").trim() {
            crate::logger::warn(&format!(
                "[youdao] 输入回显不完整: sent={} accepted={}",
                text.chars().count(),
                input.chars().count()
            ));
            return Err("有道未完整接收原文，请重新划词或切换引擎".into());
        }
    }
    parse_translation(v)
}

fn parse_translation(v: &Value) -> Result<String, String> {
    // 句子路径：fanyi.tran
    if let Some(tran) = v.pointer("/fanyi/tran").and_then(Value::as_str) {
        if !tran.is_empty() {
            return Ok(tran.to_string());
        }
    }
    // 单词和常见短语经常不返回 fanyi，而是进入网页翻译结果。
    // 只接受 @same 标记的精确词条及其第一条译文，避免误取后面的
    // 联想词条；这与 ec.word 的词典释义不同，可以安全用于引擎译文。
    if let Some(entries) = v
        .pointer("/web_trans/web-translation")
        .and_then(Value::as_array)
    {
        let exact = entries.iter().find(|entry| {
            entry
                .get("@same")
                .is_some_and(|same| same.as_str() == Some("true") || same.as_bool() == Some(true))
        });
        if let Some(tran) = exact
            .and_then(|entry| entry.get("trans"))
            .and_then(Value::as_array)
            .and_then(|translations| {
                translations
                    .iter()
                    .filter_map(|translation| translation.get("value").and_then(Value::as_str))
                    .find(|value| !value.trim().is_empty())
            })
        {
            return Ok(tran.trim().to_string());
        }
    }
    // Do not reinterpret ec.word as a text translation. Dictionary cards
    // consume that structure separately; doing it here duplicates dictionary
    // senses in the "engine translation" tab for single words.
    Err("no translation".into())
}

/// 词典查询（返回原始 JSON 供 dictionary.rs 解析）。
/// 关键段：ec.word.{usphone,ukphone,trs[].{pos,tran}}、ec.exam_type。
pub async fn lookup_raw(word: &str) -> Result<Value, String> {
    let (sgn, t) = sign(word);
    let client = super::client();
    let resp = client
        .post(ENDPOINT)
        .form(&[
            ("q", word),
            ("from", "auto"),
            ("to", "zh-CHS"),
            ("dict", "true"),
            ("t", t.as_str()),
            ("client", CLIENT),
            ("sign", sgn.as_str()),
            ("keyfrom", KEYFROM),
        ])
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    resp.json().await.map_err(|e| format!("json: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_queries_split_without_losing_source_or_breaking_unicode() {
        for input in [
            "A translator must preserve the whole text. ".repeat(20) + "Final code 7391.",
            "这是很长的中文段落，需要保留每一句。\n".repeat(80),
            "🙂".repeat(450),
            "x".repeat(1601),
        ] {
            let parts = split_query(&input, 400);
            assert_eq!(parts.concat(), input);
            assert!(parts.len() > 1);
            assert!(parts
                .iter()
                .all(|part| part.encode_utf16().count() <= 400 && !part.is_empty()));
        }
        assert_eq!(
            join_translations(
                &["First.\n", "Second."],
                &["第一段。".into(), "第二段。".into()]
            ),
            "第一段。\n第二段。"
        );
        assert_eq!(
            join_translations(
                &["第一句。", "第二句。"],
                &["First sentence.".into(), "Second sentence.".into()]
            ),
            "First sentence. Second sentence."
        );
    }

    #[test]
    fn refuses_silently_truncated_source_echo() {
        let response = serde_json::json!({"fanyi":{"input":"First sentence.","tran":"第一句。"}});
        assert!(parse_part(&response, "First sentence. Final sentence.").is_err());
        assert_eq!(
            parse_part(&response, "First sentence.").unwrap(),
            "第一句。"
        );
    }

    #[tokio::test]
    #[ignore = "Opt-in: sends fixed public fixtures to Youdao, no user selections"]
    async fn long_query_live_smoke() {
        let source =
            "A translator must preserve the whole text. ".repeat(20) + "Finally, the code is 7391.";
        let result = translate(&source, "zh-CN").await.unwrap();
        assert!(result.contains("7391"), "translation lost the final marker");
        let source = "中文翻译需要保留完整内容。".repeat(60) + "最后的编号是7391。";
        let result = translate(&source, "en").await.unwrap();
        assert!(result.contains("7391"), "translation lost the final marker");
    }

    #[tokio::test]
    #[ignore = "Opt-in: sends fixed public word and phrase fixtures to Youdao"]
    async fn word_and_phrase_live_smoke() {
        for (source, target) in [
            ("hello", "zh-CN"),
            ("hello world", "zh-CN"),
            ("How are you?", "zh-CN"),
            ("你好", "en"),
        ] {
            let result = translate(source, target).await.unwrap();
            assert!(!result.trim().is_empty(), "empty translation for {source}");
        }
    }

    #[test]
    fn text_translation_uses_exact_web_result_but_not_dictionary_senses() {
        let word = serde_json::json!({"trs":[{"pos":"v.","tran":"考虑；认为"}]});
        assert!(parse_translation(&serde_json::json!({"ec":{"word":word}})).is_err());
        let phrase = serde_json::json!({
            "web_trans": {
                "web-translation": [
                    {
                        "@same": "true",
                        "key": "hello world",
                        "trans": [
                            {"value": "你好世界"},
                            {"value": "世界你好"}
                        ]
                    },
                    {
                        "key": "Hello Kitty World",
                        "trans": [{"value": "凯蒂猫气球世界"}]
                    }
                ]
            }
        });
        assert_eq!(parse_translation(&phrase).unwrap(), "你好世界");
        let suggestions_only = serde_json::json!({
            "web_trans": {
                "web-translation": [{
                    "key": "related phrase",
                    "trans": [{"value": "不应采用"}]
                }]
            }
        });
        assert!(parse_translation(&suggestions_only).is_err());
        assert_eq!(
            parse_translation(&serde_json::json!({"fanyi":{"tran":"第一行\n最后一行"}})).unwrap(),
            "第一行\n最后一行"
        );
    }

    #[test]
    fn md5_known_vectors() {
        assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex("The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
    }
}
