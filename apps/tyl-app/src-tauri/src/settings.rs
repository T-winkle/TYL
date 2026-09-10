//! 应用设置：JSON 持久化（exe 同目录 settings.json），热生效。
//! 热键 / LLM / 代理 / 自启 / 外观与结果布局。

use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    /// 全局热键（解析失败回退默认）。
    pub hotkey: String,
    /// 启用的引擎（有序，第一个 = 主引擎/默认展示；弹窗 tab 顺序同此）。
    /// 可选 id 由 ENGINE_IDS 统一维护。
    pub engines: Vec<String>,
    /// 翻译结果展示："tabs"（按需切换）| "stacked"（全部展开并行请求）。
    pub result_display: String,
    /// "system" | "light" | "dark".
    pub theme: String,
    /// Brand palette: "jade" | "indigo" | "plum".
    pub color_scheme: String,
    /// Whether the popup shows the captured source text.
    pub show_source: bool,
    /// LLM（OpenAI 兼容）。engines 含 "llm" 时使用。
    pub llm: LlmSettings,
    /// 网络代理。
    pub proxy: ProxySettings,
    /// 词典卡片与词典数据源（与文本翻译引擎独立）。
    pub dictionary: DictionarySettings,
    /// WebView2 hardware acceleration. Applied at process startup; restart required.
    pub gpu_acceleration: bool,
    pub log_level: crate::logger::Level,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct LlmSettings {
    pub enabled: bool,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct ProxySettings {
    /// "off" | "system" | "manual"
    pub mode: String,
    /// 手动模式：http://host:port 或 socks5://host:port
    pub url: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct DictionarySettings {
    pub enabled: bool,
    /// "auto" | "youdao" | "iciba" | "bing".
    pub provider: String,
}

impl Default for DictionarySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: DICTIONARY_AUTO.into(),
        }
    }
}

// v1 stored `dictionary` as a bool. Accept both shapes so upgrades do not
// discard the rest of settings.json merely because this field evolved.
impl<'de> Deserialize<'de> for DictionarySettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Compatible {
            Boolean(bool),
            Object {
                #[serde(default = "default_true")]
                enabled: bool,
                #[serde(default = "default_dictionary_provider")]
                provider: String,
            },
        }

        Ok(match Compatible::deserialize(deserializer)? {
            Compatible::Boolean(enabled) => Self {
                enabled,
                ..Self::default()
            },
            Compatible::Object { enabled, provider } => Self { enabled, provider },
        })
    }
}

fn default_true() -> bool {
    true
}

fn default_dictionary_provider() -> String {
    DICTIONARY_AUTO.into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "alt+t".into(),
            engines: vec!["bing".into(), "youdao".into(), "transmart".into()],
            result_display: RESULT_DISPLAY_TABS.into(),
            theme: "system".into(),
            color_scheme: "indigo".into(),
            show_source: false,
            llm: LlmSettings::default(),
            proxy: ProxySettings::default(),
            dictionary: DictionarySettings::default(),
            gpu_acceleration: false,
            log_level: crate::logger::Level::Info,
        }
    }
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
        }
    }
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            mode: "system".into(),
            url: String::new(),
        }
    }
}

static SETTINGS: RwLock<Option<Settings>> = RwLock::new(None);

/// 配置文件路径：exe 同目录 settings.json（便携式，跟 exe 走）。
pub fn settings_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("settings.json")))
        .unwrap_or_else(|| std::env::temp_dir().join("tyl-settings.json"))
}

/// 合法引擎 id。
pub const ENGINE_IDS: [&str; 8] = [
    "bing",
    "youdao",
    "transmart",
    "yandex",
    "iciba",
    "google",
    "mymemory",
    "llm",
];
pub const RESULT_DISPLAY_TABS: &str = "tabs";
pub const RESULT_DISPLAY_STACKED: &str = "stacked";
pub const COLOR_SCHEMES: [&str; 3] = ["jade", "indigo", "plum"];
pub const DICTIONARY_AUTO: &str = "auto";
pub const DICTIONARY_PROVIDERS: [&str; 4] = [DICTIONARY_AUTO, "youdao", "iciba", "bing"];

/// 加载（缺失/损坏 → 默认值并写回）。启动时调用。
pub fn load() -> Settings {
    let mut s: Settings = std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Settings>(&t).ok())
        .unwrap_or_default();

    // 迁移 v1（engine: String + engines: {开关}）→ v2（engines: Vec）：
    // 旧主引擎 + 开着的对照引擎，保序；解析失败回默认。
    if s.engines.is_empty() {
        let legacy: Option<serde_json::Value> = std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
        let mut list = Vec::new();
        if let Some(v) = legacy {
            if let Some(primary) = v.get("engine").and_then(|e| e.as_str()) {
                if ENGINE_IDS.contains(&primary) {
                    list.push(primary.to_string());
                }
            }
            if let Some(cfg) = v.get("engines") {
                for id in ENGINE_IDS {
                    if cfg.get(id).and_then(|b| b.as_bool()) == Some(true)
                        && !list.iter().any(|e| e == id)
                    {
                        list.push(id.to_string());
                    }
                }
            }
        }
        if list.is_empty() {
            list = Settings::default().engines;
        }
        s.engines = list;
    }
    // 清洗：去掉非法 id 与重复
    s.engines.retain(|e| ENGINE_IDS.contains(&e.as_str()));
    s.engines.dedup();
    if s.engines.is_empty() {
        s.engines = Settings::default().engines;
    }
    normalize_dictionary(&mut s);
    if !matches!(
        s.result_display.as_str(),
        RESULT_DISPLAY_TABS | RESULT_DISPLAY_STACKED
    ) {
        s.result_display = RESULT_DISPLAY_TABS.into();
    }
    normalize_appearance(&mut s);
    *SETTINGS.write().unwrap() = Some(s.clone());
    s
}

/// 当前生效配置（未 load 时给默认）。
pub fn current() -> Settings {
    SETTINGS.read().unwrap().clone().unwrap_or_default()
}

/// 保存并生效（写文件 + 更新内存 + 触发副作用回调）。
pub fn save(mut s: Settings) -> Result<(), String> {
    normalize_appearance(&mut s);
    normalize_dictionary(&mut s);
    s.engines.retain(|e| ENGINE_IDS.contains(&e.as_str()));
    s.engines.dedup();
    if s.engines.is_empty() {
        return Err("至少需要启用一个翻译引擎".into());
    }
    if !matches!(
        s.result_display.as_str(),
        RESULT_DISPLAY_TABS | RESULT_DISPLAY_STACKED
    ) {
        s.result_display = RESULT_DISPLAY_TABS.into();
    }
    if let Err(e) = std::fs::write(
        settings_path(),
        serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?,
    ) {
        return Err(format!("写入失败: {e}"));
    }
    *SETTINGS.write().unwrap() = Some(s.clone());
    crate::logger::set_level(s.log_level);
    Ok(())
}

fn normalize_appearance(s: &mut Settings) {
    if !matches!(s.theme.as_str(), "system" | "light" | "dark") {
        s.theme = "system".into();
    }
    if !COLOR_SCHEMES.contains(&s.color_scheme.as_str()) {
        s.color_scheme = "indigo".into();
    }
}

fn normalize_dictionary(s: &mut Settings) {
    if !DICTIONARY_PROVIDERS.contains(&s.dictionary.provider.as_str()) {
        s.dictionary.provider = DICTIONARY_AUTO.into();
    }
}

impl ProxySettings {
    /// 解析代理 URL 为 reqwest 输入。None = 不显式代理。
    /// - manual：用用户填的 url（http/socks5）
    /// - system：读 Windows IE/WinINET 代理（注册表 Internet Settings），
    ///   形如 "http=127.0.0.1:7890;https=..." 或 ProxyServer 单值
    /// - off：直连
    pub fn reqwest_proxy(&self) -> Option<Result<reqwest::Proxy, String>> {
        match self.mode.as_str() {
            "manual" => {
                let url = self.url.trim();
                if url.is_empty() {
                    None
                } else {
                    Some(reqwest::Proxy::all(url).map_err(|e| format!("代理地址无效: {e}")))
                }
            }
            "system" => system_proxy()
                .map(|u| reqwest::Proxy::all(&u).map_err(|e| format!("系统代理无效: {e}"))),
            _ => None,
        }
    }
}

/// 检测当前系统代理（供 client 指纹使用；与 reqwest_proxy 的读取共享
/// 一次注册表开销）。返回 None = 未启用。
pub fn detect_system_proxy() -> Option<String> {
    system_proxy()
}

/// 读 Windows 系统代理（HKCU\...\Internet Settings）。
/// 返回 None = 系统未启用代理 → 直连。
#[cfg(target_os = "windows")]
fn system_proxy() -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::WIN32_ERROR;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };

    unsafe {
        let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        // 1) ProxyEnable (DWORD)
        let mut enabled: u32 = 0;
        let mut cb = 4u32;
        let enable_key: Vec<u16> = "ProxyEnable"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let ok = RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(enable_key.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut enabled as *mut u32 as *mut core::ffi::c_void),
            Some(&mut cb),
        );
        if ok != WIN32_ERROR(0) || enabled == 0 {
            return None; // 系统代理未开启
        }

        // 2) ProxyServer (SZ)：可能是 "host:port" 或 "http=...;https=..."
        let server_key: Vec<u16> = "ProxyServer"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut buf = [0u16; 512];
        let mut cb = (buf.len() * 2) as u32;
        let ok = RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(server_key.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut cb),
        );
        if ok != WIN32_ERROR(0) || cb == 0 {
            return None;
        }
        let len = (cb as usize / 2).saturating_sub(1); // 去尾部 NUL
        let raw = String::from_utf16_lossy(&buf[..len]);

        // 分协议形式：取 https 优先，否则 http，否则整体
        let picked = if raw.contains('=') {
            raw.split(';')
                .find_map(|p| p.strip_prefix("https="))
                .or_else(|| raw.split(';').find_map(|p| p.strip_prefix("http=")))
                .unwrap_or(&raw)
        } else {
            &raw
        }
        .to_string();

        if picked.is_empty() {
            return None;
        }
        // 无 scheme 前缀默认按 http 处理
        if picked.contains("://") {
            Some(picked)
        } else {
            Some(format!("http://{picked}"))
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn system_proxy() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_hotkey_is_alt_t() {
        assert_eq!(Settings::default().hotkey, "alt+t");
    }

    #[test]
    fn old_settings_default_to_tab_results() {
        let settings: Settings = serde_json::from_str(r#"{"hotkey":"alt+e"}"#).unwrap();
        assert_eq!(settings.result_display, RESULT_DISPLAY_TABS);
        assert_eq!(settings.color_scheme, "indigo");
        assert!(!settings.show_source);
        assert!(!settings.gpu_acceleration);
        assert_eq!(settings.log_level, crate::logger::Level::Info);
    }

    #[test]
    fn gpu_preference_round_trips_both_values() {
        for enabled in [false, true] {
            let settings = Settings {
                gpu_acceleration: enabled,
                ..Settings::default()
            };
            let restored: Settings =
                serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
            assert_eq!(restored.gpu_acceleration, enabled);
        }
    }

    #[test]
    fn retired_clipboard_policy_does_not_break_old_settings() {
        let settings: Settings = serde_json::from_str(
            r#"{"hotkey":"alt+t","clipboard_policy":"uia_only","dictionary":false}"#,
        )
        .unwrap();
        assert_eq!(settings.hotkey, "alt+t");
        assert!(!settings.dictionary.enabled);
        assert_eq!(settings.dictionary.provider, DICTIONARY_AUTO);
        assert!(serde_json::to_value(settings)
            .unwrap()
            .get("clipboard_policy")
            .is_none());
    }

    #[test]
    fn dictionary_object_round_trips_and_unknown_provider_is_normalized() {
        let settings: Settings =
            serde_json::from_str(r#"{"dictionary":{"enabled":true,"provider":"iciba"}}"#).unwrap();
        assert_eq!(settings.dictionary.provider, "iciba");
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored.dictionary, settings.dictionary);

        let mut invalid = settings;
        invalid.dictionary.provider = "removed-provider".into();
        normalize_dictionary(&mut invalid);
        assert_eq!(invalid.dictionary.provider, DICTIONARY_AUTO);
    }
}
