//! Reflection Governance — safety policies that govern whether reasoning-cycle
//! reflections are accepted and persisted as Trace memories.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::adapters::memvid_store::MemoryStore;
use super::clock::Clock;
use super::enums::{MemoryLayer, MemoryType, Scope, SourceType};
use super::errors::Result;
use super::schemas::{DurableMemory, Provenance};

// ── Candidate ────────────────────────────────────────────────────────────────

/// A single reflection candidate produced by the reasoning cycle, carrying the
/// supporting evidence required for policy evaluation.
#[derive(Debug, Clone)]
pub struct ReflectionCandidate {
    /// Human-readable reflection text.
    pub text: String,
    /// IDs of memory objects that directly support this reflection.
    pub supporting_memory_ids: Vec<String>,
    /// Confidence values of the supporting memories (parallel to `supporting_memory_ids`).
    pub supporting_confidences: Vec<f32>,
    /// Rule or analysis that produced this reflection (e.g. `"slot-frequency"`).
    pub origin_rule: String,
}

// ── Evidence threshold ────────────────────────────────────────────────────────

/// Minimum raw-evidence criteria a candidate must satisfy to enter confidence
/// scoring.
#[derive(Debug, Clone, Copy)]
pub struct ReflectionEvidenceThreshold {
    /// At least this many supporting memories are required.
    pub min_supporting_memories: usize,
    /// Mean confidence of supporting memories must meet or exceed this value.
    pub min_confidence: f32,
}

impl Default for ReflectionEvidenceThreshold {
    fn default() -> Self {
        Self {
            min_supporting_memories: 2,
            min_confidence: 0.6,
        }
    }
}

// ── Confidence score ──────────────────────────────────────────────────────────

/// Computes a composite confidence score for a candidate:
/// `mean(supporting_confidences) × coverage_factor`.
///
/// `coverage_factor` is `min(count, 5) / 5` — a breadth bonus capped at 5.
#[derive(Debug, Clone, Copy)]
pub struct ReflectionConfidenceScore;

impl ReflectionConfidenceScore {
    /// Compute the composite score for `candidate`.
    #[must_use]
    pub fn compute(candidate: &ReflectionCandidate) -> f32 {
        if candidate.supporting_confidences.is_empty() {
            return 0.0;
        }
        let mean: f32 = candidate.supporting_confidences.iter().sum::<f32>()
            / candidate.supporting_confidences.len() as f32;
        let coverage_factor = (candidate.supporting_confidences.len().min(5) as f32) / 5.0_f32;
        mean * coverage_factor
    }
}

// ── Reversibility tagging ────────────────────────────────────────────────────

/// Tags a `DurableMemory` as a reversible reflection with provenance metadata.
pub struct ReflectionReversibility;

impl ReflectionReversibility {
    /// Insert `reflection_reversible`, `reflection_origin`, and `reflection_at`
    /// into `memory.metadata`.
    pub fn tag(memory: &mut DurableMemory, origin_rule: &str, at: DateTime<Utc>) {
        memory
            .metadata
            .insert("reflection_reversible".to_string(), "true".to_string());
        memory
            .metadata
            .insert("reflection_origin".to_string(), origin_rule.to_string());
        memory
            .metadata
            .insert("reflection_at".to_string(), at.timestamp().to_string());
    }
}

// ── Decay / TTL ───────────────────────────────────────────────────────────────

/// Assigns TTL (seconds) to a persisted reflection based on evidence breadth.
pub struct ReflectionDecay;

impl ReflectionDecay {
    /// 30 days — applied to reflections with minimal evidence support (< 5 memories).
    pub const TTL_UNSUPPORTED_SECS: i64 = 30 * 86_400;
    /// 90 days — applied to well-supported reflections (≥ 5 memories).
    pub const TTL_WELL_SUPPORTED_SECS: i64 = 90 * 86_400;

    /// Select TTL: ≥ 5 supporting memories → 90 days, otherwise 30 days.
    #[must_use]
    pub fn ttl_for(candidate: &ReflectionCandidate) -> i64 {
        if candidate.supporting_memory_ids.len() >= 5 {
            Self::TTL_WELL_SUPPORTED_SECS
        } else {
            Self::TTL_UNSUPPORTED_SECS
        }
    }
}

// ── Validation outcome ────────────────────────────────────────────────────────

/// Outcome of evaluating a single `ReflectionCandidate` against policy.
#[derive(Debug, Clone)]
pub enum ValidationOutcome {
    /// Candidate passed all checks; `confidence` is the computed score.
    Pass { confidence: f32 },
    /// Candidate was rejected; `reason` explains why.
    Reject { reason: String },
}

// ── Safety policy ─────────────────────────────────────────────────────────────

/// Governs the maximum rate and quality of reflections written per cycle.
#[derive(Debug, Clone)]
pub struct ReflectionSafetyPolicy {
    /// Raw-evidence criteria applied before confidence scoring.
    pub evidence_threshold: ReflectionEvidenceThreshold,
    /// Computed confidence score must meet or exceed this value.
    pub confidence_threshold: f32,
    /// Hard cap: at most this many reflections may be written per cycle.
    pub max_reflections_per_cycle: usize,
}

impl Default for ReflectionSafetyPolicy {
    fn default() -> Self {
        Self {
            evidence_threshold: ReflectionEvidenceThreshold::default(),
            confidence_threshold: 0.6,
            max_reflections_per_cycle: 10,
        }
    }
}

// ── Filtered result ───────────────────────────────────────────────────────────

/// Result of a single validation pass over a set of reflection candidates.
#[derive(Debug, Clone)]
pub struct FilteredCycleResult {
    /// Candidates that passed all policy checks, paired with their computed
    /// confidence score.
    pub passed: Vec<(ReflectionCandidate, f32)>,
    /// Candidates that were rejected, paired with the rejection reason.
    pub rejected: Vec<(ReflectionCandidate, String)>,
}

// ── Validation layer ──────────────────────────────────────────────────────────

/// Applies `ReflectionSafetyPolicy` to a slice of candidates and optionally
/// writes accepted ones to the memory store.
pub struct ReflectionValidationLayer {
    pub policy: ReflectionSafetyPolicy,
}

impl ReflectionValidationLayer {
    #[must_use]
    pub fn new(policy: ReflectionSafetyPolicy) -> Self {
        Self { policy }
    }

    /// Validate all `candidates` against the policy.
    ///
    /// Processing stops as soon as `max_reflections_per_cycle` candidates have
    /// passed; remaining unprocessed candidates are not included in `rejected`.
    pub fn validate(&self, candidates: &[ReflectionCandidate]) -> FilteredCycleResult {
        let mut passed = Vec::new();
        let mut rejected = Vec::new();

        for candidate in candidates {
            if passed.len() >= self.policy.max_reflections_per_cycle {
                break;
            }
            match self.evaluate(candidate) {
                ValidationOutcome::Pass { confidence } => {
                    passed.push((candidate.clone(), confidence));
                }
                ValidationOutcome::Reject { reason } => {
                    rejected.push((candidate.clone(), reason));
                }
            }
        }

        FilteredCycleResult { passed, rejected }
    }

    fn evaluate(&self, candidate: &ReflectionCandidate) -> ValidationOutcome {
        let et = &self.policy.evidence_threshold;

        if candidate.supporting_memory_ids.len() < et.min_supporting_memories {
            return ValidationOutcome::Reject {
                reason: format!(
                    "insufficient evidence: {} supporting memories (minimum {})",
                    candidate.supporting_memory_ids.len(),
                    et.min_supporting_memories
                ),
            };
        }

        let mean_confidence: f32 = if candidate.supporting_confidences.is_empty() {
            0.0
        } else {
            candidate.supporting_confidences.iter().sum::<f32>()
                / candidate.supporting_confidences.len() as f32
        };

        if mean_confidence < et.min_confidence {
            return ValidationOutcome::Reject {
                reason: format!(
                    "mean evidence confidence {mean_confidence:.3} below threshold {:.3}",
                    et.min_confidence
                ),
            };
        }

        let score = ReflectionConfidenceScore::compute(candidate);
        if score < self.policy.confidence_threshold {
            return ValidationOutcome::Reject {
                reason: format!(
                    "computed confidence {score:.3} below policy threshold {:.3}",
                    self.policy.confidence_threshold
                ),
            };
        }

        ValidationOutcome::Pass { confidence: score }
    }

    /// Persist each passed candidate as a `MemoryLayer::Trace` memory with
    /// reversibility metadata and a decay TTL.
    ///
    /// Returns the `memory_id` of every written record.
    pub fn write_reflections_to_store<S: MemoryStore>(
        &self,
        filtered: &FilteredCycleResult,
        store: &mut S,
        clock: &dyn Clock,
    ) -> Result<Vec<String>> {
        let now = clock.now();
        let mut written_ids = Vec::new();

        for (candidate, confidence) in &filtered.passed {
            let memory_id = Uuid::new_v4().to_string();
            let ttl = ReflectionDecay::ttl_for(candidate);

            let mut memory = DurableMemory {
                memory_id: memory_id.clone(),
                candidate_id: Uuid::new_v4().to_string(),
                stored_at: now,
                updated_at: None,
                entity: "reflection".to_string(),
                slot: "pattern".to_string(),
                value: candidate.text.clone(),
                raw_text: candidate.text.clone(),
                memory_type: MemoryType::Trace,
                confidence: *confidence,
                salience: *confidence,
                scope: Scope::Private,
                ttl: Some(ttl),
                source: Provenance {
                    source_type: SourceType::System,
                    source_id: "reflection_governance".to_string(),
                    source_label: Some("ReflectionValidationLayer".to_string()),
                    observed_by: None,
                    trust_weight: *confidence,
                },
                event_at: None,
                valid_from: None,
                valid_to: None,
                internal_layer: Some(MemoryLayer::Trace),
                tags: vec!["reflection".to_string()],
                metadata: BTreeMap::new(),
                is_retraction: false,
            };

            ReflectionReversibility::tag(&mut memory, &candidate.origin_rule, now);
            store.put_memory(&memory)?;
            written_ids.push(memory_id);
        }

        Ok(written_ids)
    }
}
