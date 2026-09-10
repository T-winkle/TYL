//! Text capture contracts.
//!
//! The flagship feature of TYL: capturing the selected text *without*
//! trashing the system clipboard. Backends implement [`TextCapture`]; the
//! primary channel (UIA on Windows) never touches the clipboard at all, and
//! the fallback channel snapshots/restores it around a simulated copy.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Everything the capture backends need to know about the moment the user
/// triggered the action. Snapshotted *before* any fallible/side-effecting
/// work, because the fallback channel (simulated Ctrl+C) can perturb
/// foreground state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureAnchor {
    /// Physical-pixel cursor position, if retrievable.
    pub cursor: Option<crate::locator::Point>,
    /// Foreground window process name (lowercased, extension stripped),
    /// e.g. `firefox`. Used to consult the per-app policy table.
    pub target_exe: Option<String>,
}

/// Which channel produced the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureSource {
    /// Accessibility API (UIAutomation TextPattern) — zero clipboard access.
    Uia,
    /// Simulated copy with full snapshot/restore. `restored` is false when
    /// restoration was skipped (third-party wrote the clipboard in between,
    /// or the user disabled restore) — surfaced in the UI so the behavior is
    /// never a surprise.
    Clipboard { restored: bool },
    /// Typed/pasted by the user into the popup.
    Manual,
}

impl CaptureSource {
    pub fn is_pollution_free(&self) -> bool {
        matches!(self, CaptureSource::Uia | CaptureSource::Manual)
    }
}

/// Successful capture result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedText {
    pub text: String,
    /// Whether the captured range is editable. None means the backend cannot verify it.
    #[serde(default)]
    pub editable: Option<bool>,
    /// Identity of the UIA selection container. Some editable rich editors do
    /// not expose a stable ID; replacement can then use exact clipboard revalidation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_id: Option<Vec<i32>>,
    pub source: CaptureSource,
    /// Selection rectangle in physical pixels (first visible line), used to
    /// position the popup. `None` when the channel can't provide one.
    pub selection_rect: Option<crate::locator::Rect>,
    /// Time spent inside the capture backend (excluding anchor snapshot).
    pub elapsed: Duration,
}

impl CapturedText {
    /// Trimmed, collapse-whitespace view for translation requests.
    pub fn text_for_translate(&self) -> String {
        self.text.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// Capture failure modes. Ordered from most to least actionable.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// The policy table (or user setting) forbids the fallback channel for
    /// this app — e.g. conhost, where a simulated Ctrl+C is SIGINT.
    #[error("fallback channel disabled for this app ({0})")]
    FallbackDisabled(String),
    /// No channel succeeded within its deadline.
    #[error("no text captured: {0}")]
    NoText(String),
    /// The UIA channel itself misbehaved (hung thread, COM failure).
    #[error("accessibility channel error: {0}")]
    Channel(String),
}

/// Result of the capture *pipeline* (chains primary + fallback channels).
#[derive(Debug)]
pub enum CaptureOutcome {
    /// Primary channel hit.
    Primary(CapturedText),
    /// Primary missed; fallback hit after snapshot/restore.
    Fallback(CapturedText),
    /// All channels exhausted.
    Failed(CaptureError),
}

/// Platform backend contract. Implementations must be cheap to clone
/// (internally `Arc`) and safe to call from any thread; the deadline covers
/// the whole call.
pub trait TextCapture: Send + Sync {
    fn capture(
        &self,
        anchor: &CaptureAnchor,
        deadline: Duration,
    ) -> Result<CapturedText, CaptureError>;
}
