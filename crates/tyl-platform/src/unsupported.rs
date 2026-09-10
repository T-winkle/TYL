//! Stub backend for platforms without an implementation yet. Keeps the
//! workspace compiling on Linux/macOS dev machines; every call fails with a
//! clear "not implemented on this platform yet" error. Mirrors the Windows
//! backend's public surface (`WinCapture`/`WinLocator`) so downstream
//! crates compile unchanged on every platform.

use std::time::Duration;

use tyl_core::capture::CaptureOutcome;
use tyl_core::config::FallbackPolicy;
use tyl_core::{
    CaptureAnchor, CaptureError, CapturedText, MonitorInfo, ScreenLocator, TextCapture,
};

#[derive(Debug, Clone, Default)]
pub struct StubCapture;

impl TextCapture for StubCapture {
    fn capture(
        &self,
        _anchor: &CaptureAnchor,
        _deadline: Duration,
    ) -> Result<CapturedText, CaptureError> {
        Err(CaptureError::Channel(
            "platform backend not implemented yet".into(),
        ))
    }
}

#[derive(Debug, Clone, Default)]
pub struct StubLocator;

impl ScreenLocator for StubLocator {
    fn monitors(&self) -> Vec<MonitorInfo> {
        Vec::new()
    }
}

/// Mirror of the Windows `WinCapture` pipeline handle (stub behavior).
#[derive(Clone, Default)]
pub struct WinCapture;

impl WinCapture {
    pub fn with_policy(_policy: FallbackPolicy) -> Self {
        Self
    }
    pub fn with_policy_and_restore(_policy: FallbackPolicy, _restore: bool) -> Self {
        Self
    }
    pub fn capture_detailed(&self, _anchor: &CaptureAnchor, _deadline: Duration) -> CaptureOutcome {
        CaptureOutcome::Failed(CaptureError::Channel(
            "platform backend not implemented yet".into(),
        ))
    }
}

impl TextCapture for WinCapture {
    fn capture(
        &self,
        _anchor: &CaptureAnchor,
        _deadline: Duration,
    ) -> Result<CapturedText, CaptureError> {
        Err(CaptureError::Channel(
            "platform backend not implemented yet".into(),
        ))
    }
}

/// Mirror of the Windows `WinLocator` (stub behavior).
#[derive(Debug, Clone, Default)]
pub struct WinLocator;

impl ScreenLocator for WinLocator {
    fn monitors(&self) -> Vec<MonitorInfo> {
        Vec::new()
    }
}

/// Snapshot cursor + foreground process name. Always unavailable on stubs.
pub fn capture_anchor() -> CaptureAnchor {
    CaptureAnchor {
        cursor: None,
        target_exe: None,
    }
}

/// No-op on platforms without a backend.
pub fn enable_dpi_awareness() {}
