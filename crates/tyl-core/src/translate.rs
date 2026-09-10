//! Translator contracts (M3 — defined now so the capture pipeline and CLI
//! can already shape data for it; backends land in M3).

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslateRequest {
    pub text: String,
    /// BCP-47-ish source hint ("auto", "en", "zh"…).
    pub source: String,
    pub target: String,
}

/// Streaming events: traditional APIs emit one `Done`; LLMs emit `Chunk`s
/// then `Done`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranslateEvent {
    Chunk { text: String },
    Done { text: String },
    Error { message: String },
}

#[derive(Debug, Error)]
pub enum TranslateError {
    #[error("network: {0}")]
    Network(String),
    #[error("service returned {status}: {message}")]
    Service { status: u16, message: String },
    #[error("invalid service config: {0}")]
    Config(String),
}

/// A translation service. Implementations are registered in a registry
/// (M3) and driven by `ServiceConfig` entries.
pub trait Translator: Send + Sync {
    fn id(&self) -> &'static str;
    fn translate(&self, req: &TranslateRequest) -> Result<TranslateEvent, TranslateError>;
}
