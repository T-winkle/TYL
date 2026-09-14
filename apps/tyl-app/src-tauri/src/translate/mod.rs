//! 翻译服务层：多引擎（Bing 直连默认 / Google 官方 / MyMemory 直连
//! 备胎 / OpenAI 兼容 LLM 流式）。
//!
//! TLS：reqwest 走 rustls-no-provider（aws-lc-sys 交叉编译需 CMake/NASM
//! 太重），在此安装 ring provider + 平台根证书。惰性一次。

pub mod bing;
pub mod bing_dict;
pub mod dictionary;
pub mod google;
pub mod iciba;
pub mod llm;
pub mod mymemory;
pub mod transmart;
pub mod tts;
pub mod yandex;
pub mod youdao;

use std::sync::{LazyLock, OnceLock, RwLock};

use serde::Serialize;
use whatlang::{Detector, Lang};

static TLS_READY: OnceLock<()> = OnceLock::new();
static LANGUAGE_DETECTOR: LazyLock<Detector> = LazyLock::new(|| {
    Detector::with_allowlist(vec![
        Lang::Eng,
        Lang::Jpn,
        Lang::Kor,
        Lang::Fra,
        Lang::Deu,
        Lang::Spa,
        Lang::Rus,
        Lang::Por,
        Lang::Cmn,
    ])
});

/// 安装 rustls ring provider（进程一次）。在构造 reqwest Client 前调用。
fn ensure_tls() {
    TLS_READY.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// 共享 reqwest Client（连接池复用）。代理设置变更时重建
/// （client 构建后代理不可变，换代理 = 换 client）。
static CLIENT: RwLock<Option<reqwest::Client>> = RwLock::new(None);
/// 上次构建 client 用的代理指纹（检测变更）。
static CLIENT_PROXY_SIG: RwLock<String> = RwLock::new(String::new());

/// 代理设置变化后调用：丢弃旧 client，下次请求用新代理重建。
pub fn reset_client() {
    *CLIENT.write().unwrap() = None;
}

pub fn client() -> reqwest::Client {
    use rustls_platform_verifier::ConfigVerifierExt;
    ensure_tls();

    let proxy = crate::settings::current().proxy;
    // 指纹含 system 模式实际读到的代理地址：用户在 Windows 设置里
    // 开关/更换系统代理时，sig 变化 → client 重建（否则旧 client 复用
    // 到进程结束，"跟随系统"名存实亡）。
    let sys_detected = if proxy.mode == "system" {
        crate::settings::detect_system_proxy()
    } else {
        None
    };
    let sig = format!(
        "{}|{}|{}",
        proxy.mode,
        proxy.url,
        sys_detected.as_deref().unwrap_or("-")
    );
    let sig_changed = *CLIENT_PROXY_SIG.read().unwrap() != sig;
    if sig_changed {
        // 代理变了：让旧 client 失效，记录新指纹。
        *CLIENT.write().unwrap() = None;
        *CLIENT_PROXY_SIG.write().unwrap() = sig;
    }

    if let Some(c) = CLIENT.read().unwrap().clone() {
        return c;
    }
    let mut builder = reqwest::Client::builder()
        // Traditional engine total timeout. LLM overrides it per request;
        // reqwest timeout covers the entire body, not only time to first token.
        .timeout(std::time::Duration::from_secs(15))
        .use_preconfigured_tls(
            rustls::ClientConfig::with_platform_verifier().expect("platform verifier"),
        );
    match proxy.reqwest_proxy() {
        Some(Ok(p)) => {
            crate::logger::log_str(&format!("[net] HTTP client 使用代理: mode={}", proxy.mode));
            builder = builder.proxy(p);
        }
        Some(Err(_)) => {
            crate::logger::warn("[net] 代理配置无效（忽略代理，直连）");
        }
        None => {
            crate::logger::log_str(&format!("[net] HTTP client 直连（mode={}）", proxy.mode));
        }
    }
    let c = builder.build().expect("reqwest client");
    *CLIENT.write().unwrap() = Some(c.clone());
    c
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TranslationDirection {
    /// Language sent to engines. `auto` keeps provider-side detection enabled.
    pub source: String,
    /// Best-effort local detection, used for routing and the popup label.
    pub detected_source: String,
    pub target: String,
}

pub fn resolve_direction(
    text: &str,
    routing: &crate::settings::LanguageRoutingSettings,
) -> TranslationDirection {
    let detected = detect_language(text).unwrap_or_else(|| "auto".into());
    match routing.mode.as_str() {
        crate::settings::LANGUAGE_MODE_FIXED_TARGET => TranslationDirection {
            source: "auto".into(),
            detected_source: detected,
            target: routing.target.clone(),
        },
        crate::settings::LANGUAGE_MODE_FIXED_PAIR => TranslationDirection {
            source: routing.source.clone(),
            detected_source: detected,
            target: routing.target.clone(),
        },
        _ => {
            let source_is_primary = same_language_family(&detected, &routing.primary);
            TranslationDirection {
                source: "auto".into(),
                detected_source: detected,
                target: if source_is_primary {
                    routing.secondary.clone()
                } else {
                    routing.primary.clone()
                },
            }
        }
    }
}

pub fn is_supported_language(language: &str) -> bool {
    language == "auto" || crate::settings::LANGUAGE_CODES.contains(&language)
}

fn same_language_family(left: &str, right: &str) -> bool {
    left == right || left.starts_with("zh-") && right.starts_with("zh-")
}

/// Fast local detection avoids a language-detection network round trip.
pub fn detect_language(text: &str) -> Option<String> {
    let mut has_han = false;
    let mut has_kana = false;
    let mut has_hangul = false;
    for ch in text.chars() {
        has_kana |= matches!(ch, '\u{3040}'..='\u{30ff}' | '\u{31f0}'..='\u{31ff}');
        has_hangul |= matches!(ch, '\u{1100}'..='\u{11ff}' | '\u{3130}'..='\u{318f}' | '\u{ac00}'..='\u{d7af}');
        has_han |= matches!(ch, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}');
    }
    if has_kana {
        return Some("ja".into());
    }
    if has_hangul {
        return Some("ko".into());
    }
    if has_han {
        return Some("zh-CN".into());
    }

    let info = LANGUAGE_DETECTOR.detect(text)?;
    let language = match info.lang() {
        Lang::Eng => "en",
        Lang::Jpn => "ja",
        Lang::Kor => "ko",
        Lang::Fra => "fr",
        Lang::Deu => "de",
        Lang::Spa => "es",
        Lang::Rus => "ru",
        Lang::Por => "pt",
        Lang::Cmn => "zh-CN",
        _ => return None,
    };
    let letters = text.chars().filter(|ch| ch.is_alphabetic()).count();
    (info.is_reliable() || info.confidence() >= 0.55 || letters >= 16).then(|| language.to_string())
}

#[cfg(test)]
mod language_tests {
    use super::*;

    #[test]
    fn default_smart_routing_preserves_the_original_behavior() {
        let routing = crate::settings::LanguageRoutingSettings::default();
        let chinese = resolve_direction("这是中文。", &routing);
        assert_eq!(chinese.detected_source, "zh-CN");
        assert_eq!(chinese.target, "en");

        let english = resolve_direction("This is an English sentence.", &routing);
        assert_eq!(english.detected_source, "en");
        assert_eq!(english.target, "zh-CN");
    }

    #[test]
    fn smart_routing_handles_japanese_as_a_non_chinese_language() {
        let routing = crate::settings::LanguageRoutingSettings::default();
        let direction = resolve_direction("これは日本語です。", &routing);
        assert_eq!(direction.detected_source, "ja");
        assert_eq!(direction.target, "zh-CN");
    }

    #[test]
    fn detects_each_configurable_language_family_locally() {
        for (text, expected) in [
            (
                "This is a sufficiently clear sentence written in English.",
                "en",
            ),
            (
                "Ceci est une phrase suffisamment claire écrite en français.",
                "fr",
            ),
            (
                "Dies ist ein ausreichend klarer deutscher Beispielsatz.",
                "de",
            ),
            (
                "Esta es una frase suficientemente clara escrita en español.",
                "es",
            ),
            (
                "Это достаточно понятное предложение на русском языке.",
                "ru",
            ),
            (
                "Esta é uma frase suficientemente clara escrita em português.",
                "pt",
            ),
            ("한국어로 작성된 충분히 명확한 예문입니다.", "ko"),
        ] {
            assert_eq!(detect_language(text).as_deref(), Some(expected), "{text}");
        }
    }

    #[test]
    fn smart_routing_uses_the_configured_language_pair() {
        let routing = crate::settings::LanguageRoutingSettings {
            primary: "en".into(),
            secondary: "fr".into(),
            ..Default::default()
        };
        assert_eq!(
            resolve_direction("This is clearly an English sentence.", &routing).target,
            "fr"
        );
        assert_eq!(
            resolve_direction("Ceci est clairement une phrase française.", &routing).target,
            "en"
        );
        assert_eq!(
            resolve_direction("هذه جملة عربية واضحة.", &routing).target,
            "en"
        );
    }

    #[test]
    fn fixed_modes_keep_the_requested_direction() {
        let mut routing = crate::settings::LanguageRoutingSettings {
            mode: crate::settings::LANGUAGE_MODE_FIXED_TARGET.into(),
            target: "de".into(),
            ..Default::default()
        };
        let target = resolve_direction("Bonjour tout le monde", &routing);
        assert_eq!(target.source, "auto");
        assert_eq!(target.target, "de");

        routing.mode = crate::settings::LANGUAGE_MODE_FIXED_PAIR.into();
        routing.source = "fr".into();
        routing.target = "en".into();
        let pair = resolve_direction("Bonjour tout le monde", &routing);
        assert_eq!(pair.source, "fr");
        assert_eq!(pair.target, "en");
    }
}
