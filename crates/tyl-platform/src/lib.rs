//! Platform backends.
//!
//! Windows: UIA primary channel + clipboard fallback with snapshot/restore.
//! Other platforms: stubs that keep the workspace compiling until their
//! backends land (Linux X11/Wayland, macOS AX).

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "windows")]
pub use windows as backend;

#[cfg(not(target_os = "windows"))]
pub mod unsupported;

#[cfg(not(target_os = "windows"))]
pub use unsupported as backend;
