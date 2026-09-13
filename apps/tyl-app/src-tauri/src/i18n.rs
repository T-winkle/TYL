//! Lightweight native-shell localization.
//!
//! WebView copy is translated in the frontend. This module covers the tray and
//! native window title, which must be available before a page is loaded.

pub const SYSTEM: &str = "system";
pub const ZH_CN: &str = "zh-CN";
pub const EN_US: &str = "en-US";

pub fn effective_language(preference: &str) -> &'static str {
    match preference {
        ZH_CN => ZH_CN,
        EN_US => EN_US,
        _ if system_language().starts_with("zh") => ZH_CN,
        _ => EN_US,
    }
}

pub fn text<'a>(preference: &str, zh_cn: &'a str, en_us: &'a str) -> &'a str {
    if effective_language(preference) == EN_US {
        en_us
    } else {
        zh_cn
    }
}

#[cfg(target_os = "windows")]
fn system_language() -> String {
    use windows::Win32::Globalization::GetUserDefaultUILanguage;

    // The low 10 bits hold the primary language ID; 0x04 covers all Chinese
    // UI variants (Simplified and Traditional). TYL currently renders both
    // with the Simplified Chinese catalog.
    if unsafe { GetUserDefaultUILanguage() } & 0x03ff == 0x04 {
        "zh".into()
    } else {
        "en".into()
    }
}

#[cfg(not(target_os = "windows"))]
fn system_language() -> String {
    std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_language_does_not_depend_on_the_system() {
        assert_eq!(effective_language(ZH_CN), ZH_CN);
        assert_eq!(effective_language(EN_US), EN_US);
        assert_eq!(text(EN_US, "设置", "Settings"), "Settings");
    }
}
