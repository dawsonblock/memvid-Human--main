use std::collections::BTreeMap;

use uuid::Uuid;

use super::clock::Clock;
use super::enums::{MemoryType, Scope, SourceType};
use super::schemas::{CandidateMemory, Provenance};
use super::source_trust::source_weight;

/// Raw input envelope for candidate creation.
#[derive(Debug, Clone)]
pub struct RawMemoryInput {
    pub entity: Option<String>,
    pub slot: Option<String>,
    pub value: Option<String>,
    pub raw_text: String,
    pub source_type: SourceType,
    pub source_id: String,
    pub source_label: Option<String>,
    pub scope: Scope,
    pub ttl: Option<i64>,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub confidence: f32,
    pub salience: f32,
    pub is_retraction: bool,
}

/// Normalizes raw observations into governed candidates.
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryIntake;

impl MemoryIntake {
    #[must_use]
    pub fn from_raw(input: RawMemoryInput, clock: &dyn Clock) -> CandidateMemory {
        CandidateMemory {
            candidate_id: Uuid::new_v4().to_string(),
            observed_at: clock.now(),
            entity: input.entity.unwrap_or_else(|| "unknown".to_string()),
            slot: input.slot.unwrap_or_else(|| "note".to_string()),
            value: input.value.unwrap_or_else(|| input.raw_text.clone()),
            raw_text: input.raw_text,
            source: Provenance {
                source_type: input.source_type,
                source_id: input.source_id,
                source_label: input.source_label,
                observed_by: None,
                trust_weight: source_weight(input.source_type),
            },
            memory_type: MemoryType::Trace,
            confidence: input.confidence.clamp(0.0, 1.0),
            salience: input.salience.clamp(0.0, 1.0),
            scope: input.scope,
            ttl: input.ttl,
            event_at: None,
            valid_from: None,
            valid_to: None,
            tags: input.tags,
            metadata: input.metadata,
            is_retraction: input.is_retraction,
        }
    }
}
