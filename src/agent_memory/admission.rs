//! Admission controller — hard quality gate applied before any classification
//! or persistence cost is paid.

use super::enums::MemoryType;
use super::schemas::CandidateMemory;

/// Machine-readable reason for admission rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionReasonCode {
    /// Candidate confidence is below the minimum trustworthy threshold.
    LowConfidence,
    /// Candidate salience is too low to justify durable storage.
    LowSalience,
    /// Candidate raw_text is empty — nothing to store.
    EmptyContent,
}

impl AdmissionReasonCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LowConfidence => "low_confidence",
            Self::LowSalience => "low_salience",
            Self::EmptyContent => "empty_content",
        }
    }
}

/// Verdict returned by [`MemoryAdmissionController::evaluate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionVerdict {
    Accept,
    Reject(AdmissionReasonCode),
}

/// Stateless admission controller.  Evaluates a single candidate against
/// hard quality thresholds before any classification or persistence cost.
pub struct MemoryAdmissionController;

impl MemoryAdmissionController {
    /// Evaluate whether `candidate` should be admitted to the classification
    /// and persistence pipeline.
    ///
    /// `existing_count` is the number of memories already in the store.
    /// Reserved for future capacity-based policies; not used today.
    #[must_use]
    pub fn evaluate(candidate: &CandidateMemory, _existing_count: usize) -> AdmissionVerdict {
        // Fast-accept: high-priority types bypass all quality checks.
        if matches!(
            candidate.memory_type,
            MemoryType::Instruction
                | MemoryType::Constraint
                | MemoryType::Correction
                | MemoryType::Decision
        ) {
            return AdmissionVerdict::Accept;
        }

        // Fast-accept: explicit importance markers in the raw text.
        let text = candidate.raw_text.to_lowercase();
        if text.contains("remember this") || text.contains("important:") {
            return AdmissionVerdict::Accept;
        }

        // Hard-reject: empty content provides no durable signal.
        if candidate.raw_text.trim().is_empty() {
            return AdmissionVerdict::Reject(AdmissionReasonCode::EmptyContent);
        }

        // Hard-reject: confidence too low to be trustworthy.
        if candidate.confidence < 0.10 {
            return AdmissionVerdict::Reject(AdmissionReasonCode::LowConfidence);
        }

        // Hard-reject: salience too low for non-critical memory types.
        if candidate.salience < 0.20 {
            return AdmissionVerdict::Reject(AdmissionReasonCode::LowSalience);
        }

        AdmissionVerdict::Accept
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;

    use super::super::enums::{Scope, SourceType};
    use super::super::schemas::{CandidateMemory, Provenance};
    use super::*;

    fn base_candidate(text: &str) -> CandidateMemory {
        CandidateMemory {
            candidate_id: "test".to_string(),
            observed_at: Utc::now(),
            entity: None,
            slot: None,
            value: None,
            raw_text: text.to_string(),
            source: Provenance {
                source_type: SourceType::Chat,
                source_id: String::new(),
                source_label: None,
                observed_by: None,
                trust_weight: 1.0,
            },
            memory_type: MemoryType::Trace,
            confidence: 0.8,
            salience: 0.5,
            scope: Scope::Private,
            ttl: None,
            event_at: None,
            valid_from: None,
            valid_to: None,
            internal_layer: None,
            tags: Vec::new(),
            metadata: BTreeMap::new(),
            is_retraction: false,
            parent_memory_id: None,
            thread_id: None,
        }
    }

    #[test]
    fn accept_normal_candidate() {
        let c = base_candidate("I prefer dark mode");
        assert_eq!(
            MemoryAdmissionController::evaluate(&c, 0),
            AdmissionVerdict::Accept
        );
    }

    #[test]
    fn reject_low_confidence() {
        let mut c = base_candidate("something");
        c.confidence = 0.05;
        assert_eq!(
            MemoryAdmissionController::evaluate(&c, 0),
            AdmissionVerdict::Reject(AdmissionReasonCode::LowConfidence)
        );
    }

    #[test]
    fn reject_low_salience() {
        let mut c = base_candidate("hm ok");
        c.salience = 0.10;
        assert_eq!(
            MemoryAdmissionController::evaluate(&c, 0),
            AdmissionVerdict::Reject(AdmissionReasonCode::LowSalience)
        );
    }

    #[test]
    fn fast_accept_instruction_type() {
        let mut c = base_candidate("always use tabs");
        c.memory_type = MemoryType::Instruction;
        c.confidence = 0.0;
        c.salience = 0.0;
        assert_eq!(
            MemoryAdmissionController::evaluate(&c, 0),
            AdmissionVerdict::Accept
        );
    }

    #[test]
    fn fast_accept_constraint_type() {
        let mut c = base_candidate("never do x");
        c.memory_type = MemoryType::Constraint;
        c.confidence = 0.0;
        c.salience = 0.0;
        assert_eq!(
            MemoryAdmissionController::evaluate(&c, 0),
            AdmissionVerdict::Accept
        );
    }

    #[test]
    fn fast_accept_correction_type_bypasses_salience() {
        let mut c = base_candidate("actually that was wrong");
        c.memory_type = MemoryType::Correction;
        c.salience = 0.0;
        assert_eq!(
            MemoryAdmissionController::evaluate(&c, 0),
            AdmissionVerdict::Accept
        );
    }

    #[test]
    fn fast_accept_remember_this_marker() {
        let mut c = base_candidate("remember this: I prefer 2-space indent");
        c.salience = 0.05;
        assert_eq!(
            MemoryAdmissionController::evaluate(&c, 0),
            AdmissionVerdict::Accept
        );
    }

    #[test]
    fn fast_accept_important_marker() {
        let mut c = base_candidate("important: deadline is Friday");
        c.salience = 0.05;
        assert_eq!(
            MemoryAdmissionController::evaluate(&c, 0),
            AdmissionVerdict::Accept
        );
    }

    #[test]
    fn reject_empty_content() {
        let c = base_candidate("   ");
        assert_eq!(
            MemoryAdmissionController::evaluate(&c, 0),
            AdmissionVerdict::Reject(AdmissionReasonCode::EmptyContent)
        );
    }
}
