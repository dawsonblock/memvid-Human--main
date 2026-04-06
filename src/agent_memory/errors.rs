use thiserror::Error;

use crate::error::MemvidError;

/// Result alias for the governed agent-memory layer.
pub type Result<T> = std::result::Result<T, AgentMemoryError>;

/// Governed memory errors.
#[derive(Debug, Error)]
pub enum AgentMemoryError {
    #[error("invalid candidate memory: {reason}")]
    InvalidCandidate { reason: String },

    #[error("belief serialization failed: {reason}")]
    BeliefSerialization { reason: String },

    #[error("memory adapter failure: {reason}")]
    Store { reason: String },

    #[error("memvid error: {0}")]
    Memvid(#[from] MemvidError),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}
