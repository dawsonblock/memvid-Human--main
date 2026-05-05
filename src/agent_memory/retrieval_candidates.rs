//! Typed candidate pool attribution for retrieval hits.

use super::enums::MemoryLayer;
use super::schemas::RetrievalHit;

/// Which pool originally surfaced this candidate memory.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CandidatePool {
    /// Surfaced via lexical full-text search (including TF-IDF / synonym expansion).
    Lexical,
    /// Surfaced via dense embedding similarity (feature `vec` only).
    Vector,
    /// Surfaced via entity/slot metadata scan.
    Metadata,
    /// Surfaced via chronological or historical version scan.
    Time,
    /// Surfaced via the correction layer for a specific entity/slot.
    Correction,
}

/// Per-dimension score signals captured before final ranking collapse.
#[derive(Debug, Clone)]
pub struct CandidateScores {
    /// Raw lexical / content-match score (0.0 if not from lexical pool).
    pub lexical_score: f32,
    /// Dense embedding similarity score; `None` when vector pool unavailable.
    pub vector_score: Option<f32>,
    /// Whether the memory's entity and slot matched the query exactly.
    pub entity_slot_match: bool,
    /// Recency signal in [0, 1]; higher = more recent.
    pub recency: f32,
    /// Memory salience as stored.
    pub salience: f32,
    /// Memory confidence as stored.
    pub confidence: f32,
    /// Source trust weight at time of retrieval.
    pub source_trust: f32,
    /// Correction status string if applicable; `None` for non-corrections.
    pub correction_status: Option<String>,
    /// Whether the memory's scope is compatible with the query scope.
    pub scope_match: bool,
}

/// A single memory surfaced by the planner with full attribution metadata.
#[derive(Debug, Clone)]
pub struct RetrievalCandidate {
    /// The memory identifier.
    pub memory_id: String,
    /// The resolved memory layer for this candidate.
    pub layer: MemoryLayer,
    /// One or more pools that surfaced this memory (de-duplicated).
    pub source_pools: Vec<CandidatePool>,
    /// Per-dimension score breakdown used for ranking.
    pub scores: CandidateScores,
    /// The final governed retrieval hit forwarded upstream.
    pub hit: RetrievalHit,
    /// Human-readable rationale lines.
    pub reason: Vec<String>,
}
