//! 单词词典：有道、金山和 Bing 是独立数据源，不与文本翻译引擎绑定。
//!
//! 旧方案（Google dt=bd）废弃：官方端点需代理，镜像端点 dt=bd 空。
//! dictionaryapi.dev（英文音标）被有道替代：同为中国直连且数据更全。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 判断取到的文本是否"像一个单词"（该走词典而非翻译）。
/// 规则：单个 token、纯字母、长度 2-24、不含空格/标点。
pub fn is_single_word(text: &str) -> bool {
    let t = text.trim();
    if t.len() < 2 || t.len() > 24 {
        return false;
    }
    !t.is_empty() && t.chars().all(|c| c.is_ascii_alphabetic()) && t.split_whitespace().count() == 1
}

/// 词典卡片数据（推给前端渲染）。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DictEntry {
    pub word: String,
    /// 美音音标（有道 usphone）
    pub phonetic_us: Option<String>,
    /// 英音音标（有道 ukphone）
    pub phonetic_uk: Option<String>,
    /// 分词性分组的释义
    pub senses: Vec<SenseGroup>,
    /// 考试标签（CET4/CET6/考研…）
    pub exam_types: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SenseGroup {
    /// "v." / "n." / "adj." …
    pub pos: String,
    /// 该词性下的释义
    pub definitions: Vec<String>,
}

/// 查词典。`auto` 按有道 → 金山 → Bing 降级；显式选择时只请求该服务。
/// 整链 6s 上限，避免增强型词典功能阻塞翻译主流程。
pub async fn lookup(word: &str, provider: &str) -> Result<DictEntry, String> {
    let fut = async {
        if provider != crate::settings::DICTIONARY_AUTO {
            return lookup_once(word, provider).await;
        }
        // Bound every attempt as well as the whole chain. A stalled primary
        // must not consume the entire budget and prevent real fallback.
        for candidate in ["youdao", "iciba", "bing"] {
            if let Ok(Ok(entry)) = tokio::time::timeout(
                std::time::Duration::from_millis(1_900),
                lookup_once(word, candidate),
            )
            .await
            {
                return Ok(entry);
            }
        }
        Err("no dictionary data".into())
    };
    match tokio::time::timeout(std::time::Duration::from_secs(6), fut).await {
        Ok(res) => res,
        Err(_) => Err("dict timeout".into()),
    }
}

async fn lookup_once(word: &str, provider: &str) -> Result<DictEntry, String> {
    let result = match provider {
        "youdao" => lookup_youdao(word).await,
        "iciba" => lookup_iciba(word).await,
        "bing" => lookup_bing(word).await,
        _ => return Err("unknown dictionary provider".into()),
    };
    require_senses(result)
}

fn require_senses(result: Result<DictEntry, String>) -> Result<DictEntry, String> {
    result.and_then(|entry| {
        if entry.senses.is_empty() {
            Err("no dictionary data".into())
        } else {
            Ok(entry)
        }
    })
}

/// 有道词典。ec 段：word.{usphone,ukphone,trs[].{pos,tran}}。
async fn lookup_youdao(word: &str) -> Result<DictEntry, String> {
    let v = super::youdao::lookup_raw(word).await?;
    let mut entry = DictEntry {
        word: word.to_string(),
        ..Default::default()
    };

    // ec.word 兼容两种形态：单词条时是对象，多词条时是数组（实测单词=对象）
    let w = match v.pointer("/ec/word") {
        Some(Value::Object(_)) => v.pointer("/ec/word"),
        Some(Value::Array(a)) => a.first(),
        _ => None,
    };
    if let Some(w) = w {
        entry.phonetic_us = w
            .pointer("/usphone")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        entry.phonetic_uk = w
            .pointer("/ukphone")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if let Some(trs) = w.pointer("/trs").and_then(Value::as_array) {
            for tr in trs.iter().take(20) {
                let pos = tr.pointer("/pos").and_then(Value::as_str).unwrap_or("");
                let def = tr
                    .pointer("/tran")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if def.is_empty() {
                    continue;
                }
                entry.senses.push(SenseGroup {
                    pos: if pos.is_empty() {
                        "".into()
                    } else {
                        pos.into()
                    },
                    definitions: def
                        .split('；')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect(),
                });
            }
        }
    }

    if let Some(exams) = v.pointer("/ec/exam_type").and_then(Value::as_array) {
        entry.exam_types = exams
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
    }
    Ok(entry)
}

/// 金山词典页的 Next.js 预加载数据。
async fn lookup_iciba(word: &str) -> Result<DictEntry, String> {
    let value = super::iciba::lookup_raw(word).await?;
    parse_iciba(&value, word)
}

fn parse_iciba(value: &Value, word: &str) -> Result<DictEntry, String> {
    let root = value
        .pointer("/props/pageProps/initialReduxState/word/wordInfo/baesInfo")
        .or_else(|| value.pointer("/pageProps/initialReduxState/word/wordInfo/baesInfo"))
        .ok_or("no dictionary data")?;
    let returned_word = root.get("word_name").and_then(Value::as_str).unwrap_or("");
    if !returned_word.eq_ignore_ascii_case(word) && !exchange_contains(root.get("exchange"), word) {
        return Err("dictionary word mismatch".into());
    }

    let mut entry = DictEntry {
        word: returned_word.into(),
        ..Default::default()
    };
    let Some(symbol) = root
        .get("symbols")
        .and_then(Value::as_array)
        .and_then(|symbols| symbols.first())
    else {
        return Ok(entry);
    };
    entry.phonetic_uk = non_empty_string(symbol.get("ph_en"));
    entry.phonetic_us = non_empty_string(symbol.get("ph_am"));
    if let Some(parts) = symbol.get("parts").and_then(Value::as_array) {
        for part in parts.iter().take(20) {
            let definitions = part
                .get("means")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|definition| !definition.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            if !definitions.is_empty() {
                entry.senses.push(SenseGroup {
                    pos: part
                        .get("part")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .into(),
                    definitions,
                });
            }
        }
    }
    Ok(entry)
}

fn exchange_contains(value: Option<&Value>, expected: &str) -> bool {
    match value {
        Some(Value::String(form)) => form.eq_ignore_ascii_case(expected),
        Some(Value::Array(forms)) => forms
            .iter()
            .any(|form| exchange_contains(Some(form), expected)),
        Some(Value::Object(forms)) => forms
            .values()
            .any(|form| exchange_contains(Some(form), expected)),
        _ => false,
    }
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Bing 词典兜底。tlookupv3：[{translations:[{posTag,displayTarget}]}]。
async fn lookup_bing(word: &str) -> Result<DictEntry, String> {
    let v = super::bing_dict::lookup_raw(word).await?;
    let mut entry = DictEntry {
        word: word.to_string(),
        ..Default::default()
    };
    let translations = v
        .pointer("/0/translations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut order: Vec<String> = Vec::new();
    let mut grouped: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for t in &translations {
        let pos = t
            .pointer("/posTag")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let def = t
            .pointer("/displayTarget")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if pos.is_empty() || def.is_empty() {
            continue;
        }
        let e = grouped.entry(pos.clone()).or_insert_with(|| {
            order.push(pos.clone());
            Vec::new()
        });
        if e.len() < 3 {
            e.push(def);
        }
    }
    for pos in order {
        if let Some(defs) = grouped.remove(&pos) {
            entry.senses.push(SenseGroup {
                pos,
                definitions: defs,
            });
        }
    }

    if entry.senses.is_empty() {
        return Err("no dictionary data".into());
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iciba_phonetics_senses_and_inflected_queries() {
        let value = serde_json::json!({
            "props":{"pageProps":{"initialReduxState":{"word":{"wordInfo":{"baesInfo":{
                "word_name":"example",
                "exchange":{"word_pl":["examples"]},
                "symbols":[{"ph_en":"ɪɡˈzɑːmpl","ph_am":"ɪɡˈzæmpl","parts":[
                    {"part":"n.","means":["例子","范例"]}
                ]}]
            }}}}}}
        });
        let entry = parse_iciba(&value, "examples").unwrap();
        assert_eq!(entry.word, "example");
        assert_eq!(entry.phonetic_uk.as_deref(), Some("ɪɡˈzɑːmpl"));
        assert_eq!(entry.senses[0].definitions, ["例子", "范例"]);
        assert!(parse_iciba(&value, "unrelated").is_err());
    }
}
