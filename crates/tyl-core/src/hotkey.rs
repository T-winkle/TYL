//! Hotkey string parsing ("Ctrl+Alt+T" style), shared by the config layer
//! and every platform registration path.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Modifier {
    Alt,
    Ctrl,
    Shift,
    Meta,
}

impl Modifier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Modifier::Alt => "Alt",
            Modifier::Ctrl => "Ctrl",
            Modifier::Shift => "Shift",
            Modifier::Meta => "Meta",
        }
    }
}

/// A parsed hotkey: modifier set + main key (letter, digit or F-key).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hotkey {
    #[serde(default)]
    pub modifiers: Vec<Modifier>,
    pub key: String,
}

#[derive(Debug, Error, PartialEq)]
pub enum HotkeyError {
    #[error("empty hotkey")]
    Empty,
    #[error("missing main key (only modifiers given)")]
    NoKey,
    #[error("invalid key: {0}")]
    InvalidKey(String),
}

impl Hotkey {
    /// Parses "ctrl+alt+t", "Ctrl+Shift+F1", "alt+q" (case-insensitive).
    pub fn parse(s: &str) -> Result<Self, HotkeyError> {
        let mut modifiers = Vec::new();
        let mut key: Option<String> = None;
        for part in s.split('+') {
            let p = part.trim();
            if p.is_empty() {
                return Err(HotkeyError::Empty);
            }
            match p.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers.push(Modifier::Ctrl),
                "alt" | "option" => modifiers.push(Modifier::Alt),
                "shift" => modifiers.push(Modifier::Shift),
                "meta" | "cmd" | "super" | "win" => modifiers.push(Modifier::Meta),
                lower => {
                    if key.is_some() {
                        return Err(HotkeyError::InvalidKey(s.to_string()));
                    }
                    // Single alnum char or F1–F24.
                    let valid = (lower.len() == 1
                        && (lower.chars().all(|c| c.is_ascii_alphanumeric())))
                        || (lower.len() <= 3
                            && lower.starts_with('f')
                            && lower[1..].chars().all(|c| c.is_ascii_digit())
                            && lower[1..]
                                .parse::<u8>()
                                .map(|n| (1..=24).contains(&n))
                                .unwrap_or(false));
                    if !valid {
                        return Err(HotkeyError::InvalidKey(p.to_string()));
                    }
                    key = Some(lower.to_uppercase());
                }
            }
        }
        Ok(Hotkey {
            modifiers,
            key: key.ok_or(HotkeyError::NoKey)?,
        })
    }

    /// Canonical display form, e.g. "Ctrl+Alt+T".
    pub fn to_display(&self) -> String {
        let mut parts: Vec<String> = self
            .modifiers
            .iter()
            .map(|m| m.as_str().to_string())
            .collect();
        parts.push(self.key.clone());
        parts.join("+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic() {
        let h = Hotkey::parse("ctrl+alt+t").unwrap();
        assert_eq!(h.modifiers, vec![Modifier::Ctrl, Modifier::Alt]);
        assert_eq!(h.key, "T");
        assert_eq!(h.to_display(), "Ctrl+Alt+T");
    }

    #[test]
    fn parses_fkeys_and_case() {
        assert_eq!(Hotkey::parse("Shift+f3").unwrap().key, "F3");
        assert_eq!(Hotkey::parse("alt+q").unwrap().key, "Q");
        assert_eq!(Hotkey::parse("Meta+1").unwrap().key, "1");
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(Hotkey::parse(""), Err(HotkeyError::Empty));
        assert_eq!(Hotkey::parse("ctrl+alt"), Err(HotkeyError::NoKey));
        assert!(matches!(
            Hotkey::parse("ctrl+xyz"),
            Err(HotkeyError::InvalidKey(_))
        ));
        assert!(matches!(
            Hotkey::parse("f25"),
            Err(HotkeyError::InvalidKey(_))
        ));
        assert!(matches!(Hotkey::parse("ctrl++"), Err(HotkeyError::Empty)));
    }
}
