//! Platform backends.
//!
//! Windows: UIA primary channel + clipboard fallback with snapshot/restore.
//! macOS: Accessibility API capture in global screen points.
//! Other platforms: buildable stubs.

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub use macos as backend;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "windows")]
pub use windows as backend;

#[cfg(target_os = "windows")]
pub use windows::{WinCapture as Capture, WinLocator as Locator};

#[cfg(target_os = "macos")]
pub use macos::{MacCapture as Capture, MacLocator as Locator};

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub mod unsupported;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub use unsupported as backend;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub use unsupported::{WinCapture as Capture, WinLocator as Locator};
