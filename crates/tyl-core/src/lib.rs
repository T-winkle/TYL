//! Platform-neutral contracts for TYL.
//!
//! Everything in this crate compiles on all targets and knows nothing about
//! Win32/UIA/AX/Wayland — platform backends live in `tyl-platform` and only
//! depend *into* this crate.

pub mod capture;
pub mod config;
pub mod hotkey;
pub mod locator;
pub mod translate;

pub use capture::{CaptureAnchor, CaptureError, CaptureSource, CapturedText, TextCapture};
pub use config::AppConfig;
pub use locator::{MonitorInfo, Rect, ScreenLocator, Size};
pub use translate::{TranslateError, TranslateEvent, TranslateRequest, Translator};
pub mod text;
