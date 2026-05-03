use super::super::enums::MemoryType;
use super::super::schemas::CandidateMemory;

/// Adjusts the `confidence` and `salience` scores of a [`CandidateMemory`]
/// based on the structural completeness of the extracted data.
#[derive(Debug, Default, Clone)]
pub struct CandidateScorer;

impl CandidateScorer {
    /// Raises confidence and salience floors according to match quality:
    /// * Full SVO triple (entity + slot + value) → confidence ≥ 0.7
    /// * Partial structure (entity + one of slot/value) → confidence ≥ 0.4
    /// * Skill/procedure candidates always reach confidence ≥ 0.65
    pub fn score(&self, candidate: &mut CandidateMemory) {
        let has_entity = candidate
            .entity
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        let has_slot = candidate
            .slot
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        let has_value = candidate
            .value
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);

        if has_entity && has_slot && has_value {
            if candidate.confidence < 0.7 {
                candidate.confidence = 0.7;
            }
        } else if has_entity && (has_slot || has_value) {
            if candidate.confidence < 0.4 {
                candidate.confidence = 0.4;
            }
        }

        if matches!(candidate.memory_type, MemoryType::Skill) {
            if candidate.salience < 0.65 {
                candidate.salience = 0.65;
            }
            if candidate.confidence < 0.65 {
                candidate.confidence = 0.65;
            }
        }

        // Salience floor: never below half of confidence.
        if candidate.salience < candidate.confidence * 0.5 {
            candidate.salience = candidate.confidence * 0.5;
        }
    }
}
