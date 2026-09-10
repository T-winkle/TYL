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

use std::sync::{OnceLock, RwLock};

static TLS_READY: OnceLock<()> = OnceLock::new();

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

/// 翻译目标语言判定：中文 → 英，其他 → 中。
pub fn auto_target(text: &str) -> &'static str {
    let has_cjk = text.chars().any(|c| {
        ('\u{4E00}'..='\u{9FFF}').contains(&c)      // CJK 统一表意
            || ('\u{3400}'..='\u{4DBF}').contains(&c) // 扩展 A
            || ('\u{F900}'..='\u{FAFF}').contains(&c) // 兼容表意
    });
    if has_cjk {
        "en"
    } else {
        "zh-CN"
    }
}
