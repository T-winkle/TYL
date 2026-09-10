//! Typed configuration model. Persisted as JSON via the store plugin in the
//! app; versioned so future migrations have a hook point.

use serde::{Deserialize, Serialize};

use crate::hotkey::Hotkey;

pub const CONFIG_VERSION: u32 = 1;

/// How the fallback (clipboard) channel may be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPolicy {
    /// UIA first, then clipboard fallback with snapshot/restore. Default.
    #[default]
    Auto,
    /// Never touch the clipboard — absolute no-pollution mode.
    UiaOnly,
    /// Skip UIA entirely (for apps where UIA misbehaves).
    ClipboardOnly,
}

/// Per-app overrides keyed by process name (lowercase, no extension).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AppPolicy {
    /// Force the clipboard channel for these processes.
    #[serde(default)]
    pub force_clipboard: Vec<String>,
    /// Never use the clipboard channel for these processes (conhost etc.).
    #[serde(default)]
    pub deny_clipboard: Vec<String>,
}

impl AppPolicy {
    /// Built-in guards: in terminals a simulated Ctrl+C is SIGINT (kills the
    /// running program — or ourselves), so they default to UIA-only.
    pub fn with_builtin() -> Self {
        Self {
            force_clipboard: Vec::new(),
            deny_clipboard: [
                "windowsterminal",
                "wt",
                "conhost",
                "openconsole",
                "microsoft.console",
                "cmd",
                "powershell",
                "pwsh",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub id: String,
    pub enabled: bool,
    /// Translator-specific payload (api key ref, base url, model…).
    #[serde(default)]
    pub options: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub version: u32,
    /// Global hotkey, e.g. "Ctrl+Alt+T" (parsed on load; invalid → default).
    pub hotkey: String,
    pub fallback_policy: FallbackPolicy,
    #[serde(default = "default_true")]
    pub restore_clipboard: bool,
    #[serde(default)]
    pub popup_margin: i32,
    #[serde(default)]
    pub app_policies: AppPolicy,
    #[serde(default)]
    pub services: Vec<ServiceConfig>,
}

fn default_true() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            hotkey: "Ctrl+Alt+T".into(),
            fallback_policy: FallbackPolicy::Auto,
            restore_clipboard: true,
            popup_margin: 8,
            app_policies: AppPolicy::with_builtin(),
            services: vec![
                ServiceConfig {
                    id: "google".into(),
                    enabled: true,
                    options: serde_json::json!({}),
                },
                ServiceConfig {
                    id: "llm".into(),
                    enabled: false,
                    options: serde_json::json!({}),
                },
            ],
        }
    }
}

impl AppConfig {
    /// Resolves the effective fallback policy for a target process,
    /// combining the global setting with the per-app table.
    pub fn effective_policy(&self, target_exe: Option<&str>) -> FallbackPolicy {
        let Some(exe) = target_exe.map(str::to_ascii_lowercase) else {
            return self.fallback_policy;
        };
        let apps = &self.app_policies;
        if apps.deny_clipboard.iter().any(|p| p == &exe) {
            return FallbackPolicy::UiaOnly;
        }
        if apps.force_clipboard.iter().any(|p| p == &exe) {
            return FallbackPolicy::ClipboardOnly;
        }
        self.fallback_policy
    }

    pub fn parse_hotkey(&self) -> Hotkey {
        Hotkey::parse(&self.hotkey).unwrap_or(Hotkey {
            modifiers: vec![crate::hotkey::Modifier::Ctrl, crate::hotkey::Modifier::Alt],
            key: "T".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_denies_conhost() {
        let cfg = AppConfig::default();
        // Case-insensitive on the process name, extension-insensitive.
        assert_eq!(
            cfg.effective_policy(Some("conhost")),
            FallbackPolicy::UiaOnly
        );
        assert_eq!(
            cfg.effective_policy(Some("CONHOST")),
            FallbackPolicy::UiaOnly
        );
    }

    #[test]
    fn per_app_override() {
        let mut cfg = AppConfig::default();
        cfg.app_policies.force_clipboard.push("firefox".into());
        assert_eq!(
            cfg.effective_policy(Some("firefox")),
            FallbackPolicy::ClipboardOnly
        );
        assert_eq!(cfg.effective_policy(Some("chrome")), FallbackPolicy::Auto);
        assert_eq!(cfg.effective_policy(None), FallbackPolicy::Auto);
    }

    #[test]
    fn hotkey_falls_back_to_default_on_invalid() {
        let cfg = AppConfig {
            hotkey: "not a hotkey".into(),
            ..Default::default()
        };
        assert_eq!(cfg.parse_hotkey().to_display(), "Ctrl+Alt+T");
    }
}
