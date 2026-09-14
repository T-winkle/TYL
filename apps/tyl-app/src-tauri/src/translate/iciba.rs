//! iCIBA internal web services. These endpoints require no user credentials,
//! but are not a public API with an availability guarantee.

use futures_util::{stream, StreamExt, TryStreamExt};
use serde_json::Value;

const TRANSLATE_ENDPOINT: &str = "https://dictionary.iciba.com/dictionary/fy/batch";
const TRANSLATE_PATH: &str = "/dictionary/fy/batch";
const CLIENT: &str = "6";
const KEY: &str = "1000006";
const SIGNATURE_SALT: &str = "7ece94d9f9c202b0d2ec557dg4r9bc";
const WORD_PAGE: &str = "https://www.iciba.com/word";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/127 Safari/537.36";

pub async fn translate(text: &str, source: &str, target: &str) -> Result<String, String> {
    // The batch endpoint starts failing around 4k characters. A conservative
    // boundary leaves headroom for non-ASCII input and future server changes.
    let parts = split_text(text, 1_600);
    let translated: Vec<String> = stream::iter(parts.iter().copied())
        .map(|part| async move {
            if part.trim().is_empty() {
                Ok(String::new())
            } else {
                translate_part(part.trim(), source, target).await
            }
        })
        .buffered(3)
        .try_collect()
        .await?;
    Ok(join_translations(&parts, &translated))
}

async fn translate_part(text: &str, source: &str, target: &str) -> Result<String, String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
        .to_string();
    let raw = format!("{TRANSLATE_PATH}{CLIENT}{KEY}{timestamp}{SIGNATURE_SALT}");
    let signature = super::youdao::md5_hex(&raw);
    let target = normalize_language(target);
    // Preserve the historically more reliable explicit Chinese direction;
    // other automatic sources remain provider-detected.
    let source = if source == "auto" && contains_han(text) {
        "zh"
    } else {
        normalize_language(source)
    };
    let payload = serde_json::json!({
        "from": source,
        "to": target,
        "textList": [text],
    });
    let response = super::client()
        .post(TRANSLATE_ENDPOINT)
        .query(&[
            ("client", CLIENT),
            ("key", KEY),
            ("timestamp", timestamp.as_str()),
            ("signature", signature.as_str()),
        ])
        .header("Origin", "https://www.iciba.com")
        .header("Referer", "https://www.iciba.com/")
        .header("User-Agent", USER_AGENT)
        .json(&payload)
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
    let success = value
        .get("code")
        .is_some_and(|code| code.as_i64() == Some(1) || code.as_str() == Some("1"));
    if !success {
        return Err(format!(
            "金山翻译返回错误码 {}",
            value
                .get("code")
                .map(Value::to_string)
                .unwrap_or_else(|| "unknown".into())
        ));
    }
    let output = value
        .pointer("/data/0/out")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/data/0").and_then(Value::as_str))
        .unwrap_or("")
        .trim();
    if output.is_empty() {
        Err("金山未返回译文".into())
    } else {
        Ok(output.into())
    }
}

pub async fn lookup_raw(word: &str) -> Result<Value, String> {
    let response = super::client()
        .get(WORD_PAGE)
        .query(&[("w", word)])
        .header("Referer", "https://www.iciba.com/")
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|error| format!("network: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("http {}", response.status()));
    }
    let html = response
        .text()
        .await
        .map_err(|error| format!("body: {error}"))?;
    const START: &str = "<script id=\"__NEXT_DATA__\" type=\"application/json\">";
    let json = if html.trim_start().starts_with('{') {
        html.as_str()
    } else {
        let start = html.find(START).ok_or("金山词典页面结构已变更")? + START.len();
        let end = html[start..]
            .find("</script>")
            .map(|offset| start + offset)
            .ok_or("金山词典数据不完整")?;
        &html[start..end]
    };
    serde_json::from_str(json).map_err(|error| format!("json: {error}"))
}

fn normalize_language(language: &str) -> &str {
    match language {
        "zh-CN" => "zh",
        "zh-TW" => "cht",
        other => other,
    }
}

fn contains_han(text: &str) -> bool {
    text.chars().any(|ch| {
        matches!(ch, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}')
    })
}

pub(crate) fn split_text(mut text: &str, limit: usize) -> Vec<&str> {
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
            if ".!?;。！？；\n".contains(ch) {
                sentence = end;
            }
        }
        if end < text.len() {
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

pub(crate) fn join_translations(parts: &[&str], translated: &[String]) -> String {
    let mut output = String::new();
    for (index, result) in translated.iter().enumerate() {
        if index > 0 && !output.is_empty() && !result.is_empty() {
            let before = parts[index - 1];
            let after = parts[index];
            let boundary = format!(
                "{}{}",
                &before[before.trim_end().len()..],
                &after[..after.len() - after.trim_start().len()]
            );
            let newlines = boundary.chars().filter(|ch| *ch == '\n').count();
            if newlines > 0 {
                output.extend(std::iter::repeat_n('\n', newlines));
            } else if !(output.ends_with(char::is_whitespace)
                || result.chars().next().is_some_and(is_cjk))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_preserves_all_unicode_input_and_natural_newlines() {
        let input = format!(
            "{}\n{} FINAL7391",
            "First sentence. ".repeat(200),
            "中文段落。".repeat(200)
        );
        let parts = split_text(&input, 1_600);
        assert_eq!(parts.concat(), input);
        assert!(parts.len() > 1);
        assert!(parts
            .iter()
            .all(|part| part.encode_utf16().count() <= 1_600));
        assert_eq!(
            join_translations(&["one\n", "two"], &["一".into(), "二".into()]),
            "一\n二"
        );
    }

    #[tokio::test]
    #[ignore = "Opt-in: sends fixed public fixtures to iCIBA"]
    async fn live_translation_and_dictionary_smoke() {
        let translated = translate(
            &("Every segment must remain complete. ".repeat(100) + "Final marker 7391."),
            "auto",
            "zh-CN",
        )
        .await
        .unwrap();
        assert!(translated.contains("7391"));
        let dictionary = lookup_raw("example").await.unwrap();
        assert_eq!(
            dictionary
                .pointer("/props/pageProps/initialReduxState/word/wordInfo/baesInfo/word_name")
                .and_then(Value::as_str),
            Some("example")
        );
    }
}
