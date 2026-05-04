//! Feedback-driven weight adjustment for the retrieval ranker.
//!
//! [`FeedbackAdjuster`] accepts per-result feedback polarities and
//! incrementally nudges the [`SoftWeights`] used by the
//! [`crate::agent_memory::ranker::Ranker`].  The adjustment is purely
//! additive with clamping — no external ML, no sidecar files.

use super::policy::SoftWeights;
use super::ranker::{
    SCORE_COMPONENT_CONTENT_MATCH_KEY, SCORE_COMPONENT_EVIDENCE_STRENGTH_KEY,
    SCORE_COMPONENT_GOAL_RELEVANCE_KEY, SCORE_COMPONENT_PROCEDURE_SUCCESS_KEY,
    SCORE_COMPONENT_RECENCY_KEY, SCORE_COMPONENT_SALIENCE_KEY, SCORE_COMPONENT_SELF_RELEVANCE_KEY,
    SCORE_COMPONENT_TOTAL_KEY,
};
use super::schemas::RetrievalHit;

/// Minimum allowed value for any tunable soft weight.
pub const MIN_WEIGHT: f32 = 0.05;
/// Maximum allowed value for any tunable soft weight.
pub const MAX_WEIGHT: f32 = 2.0;

/// A score component must contribute at least this fraction of the total
/// absolute score before its corresponding weight is nudged.
const DOMINANCE_THRESHOLD: f32 = 0.15;

/// Score component metadata keys that map to adjustable [`SoftWeights`] fields.
///
/// `SCORE_COMPONENT_SEMANTIC_SCORE_KEY` is intentionally absent: the semantic
/// score shares `content_match` weight in the ranker (`score_breakdown`), so
/// adjusting `content_match` covers both signals.
const ADJUSTABLE_COMPONENTS: &[&str] = &[
    SCORE_COMPONENT_CONTENT_MATCH_KEY,
    SCORE_COMPONENT_GOAL_RELEVANCE_KEY,
    SCORE_COMPONENT_SELF_RELEVANCE_KEY,
    SCORE_COMPONENT_SALIENCE_KEY,
    SCORE_COMPONENT_EVIDENCE_STRENGTH_KEY,
    SCORE_COMPONENT_RECENCY_KEY,
    SCORE_COMPONENT_PROCEDURE_SUCCESS_KEY,
];

/// Adjusts [`SoftWeights`] incrementally based on per-result feedback signals.
///
/// Positive polarity (`+1.0`) increases the weights of score components that
/// dominated the returned hit.  Negative polarity (`-1.0`) decreases them.
/// All weights remain in `[MIN_WEIGHT, MAX_WEIGHT]` after every update.
///
/// # Example
///
/// ```no_run
/// use memvid_core::agent_memory::policy::PolicyProfile;
/// use memvid_core::agent_memory::retrieval_feedback::FeedbackAdjuster;
///
/// let baseline = PolicyProfile::default().soft_weights().clone();
/// let mut adjuster = FeedbackAdjuster::new(baseline);
/// // … retrieve some hits, then:
/// // adjuster.apply(&top_hit, 1.0);   // result was helpful
/// // adjuster.apply(&poor_hit, -1.0); // result was irrelevant
/// let _adjusted = adjuster.weights().clone();
/// ```
#[derive(Debug, Clone)]
pub struct FeedbackAdjuster {
    weights: SoftWeights,
    learning_rate: f32,
    feedback_count: u32,
}

impl FeedbackAdjuster {
    /// Default per-feedback step size applied to each dominant component.
    pub const DEFAULT_LEARNING_RATE: f32 = 0.05;

    /// Create a new adjuster seeded from `baseline` with
    /// [`Self::DEFAULT_LEARNING_RATE`].
    #[must_use]
    pub fn new(baseline: SoftWeights) -> Self {
        Self::with_learning_rate(baseline, Self::DEFAULT_LEARNING_RATE)
    }

    /// Create a new adjuster with a custom learning rate clamped to `[0, 1]`.
    #[must_use]
    pub fn with_learning_rate(baseline: SoftWeights, learning_rate: f32) -> Self {
        Self {
            weights: baseline,
            learning_rate: learning_rate.abs().min(1.0),
            feedback_count: 0,
        }
    }

    /// Return the current adjusted weights.
    #[must_use]
    pub const fn weights(&self) -> &SoftWeights {
        &self.weights
    }

    /// Return the number of feedback signals that have been applied.
    #[must_use]
    pub const fn feedback_count(&self) -> u32 {
        self.feedback_count
    }

    /// Apply a feedback signal derived from a single retrieval hit.
    ///
    /// `polarity` is `+1.0` for a helpful result and `-1.0` for an unhelpful
    /// one.  Intermediate values are accepted and scaled proportionally.
    ///
    /// The method reads the `score_component_*` metadata keys written by
    /// [`crate::agent_memory::ranker::Ranker::rerank_with_weights`].  Any
    /// component whose absolute contribution exceeds [`DOMINANCE_THRESHOLD`]
    /// of the total absolute score has its corresponding weight nudged by
    /// `learning_rate × polarity`, then clamped to `[MIN_WEIGHT, MAX_WEIGHT]`.
    ///
    /// If the hit carries no scored component data the call is a no-op.
    pub fn apply(&mut self, hit: &RetrievalHit, polarity: f32) {
        let total_abs = hit
            .metadata
            .get(SCORE_COMPONENT_TOTAL_KEY)
            .and_then(|v| v.parse::<f32>().ok())
            .map(f32::abs)
            .unwrap_or(0.0);

        if total_abs < f32::EPSILON {
            return;
        }

        self.feedback_count = self.feedback_count.saturating_add(1);
        let delta = self.learning_rate * polarity;

        for &key in ADJUSTABLE_COMPONENTS {
            let contribution = hit
                .metadata
                .get(key)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0);

            if contribution.abs() / total_abs > DOMINANCE_THRESHOLD {
                Self::nudge_weight(&mut self.weights, key, delta);
            }
        }
    }

    /// Apply feedback from a slice of hits, processed in order.
    pub fn apply_all(&mut self, hits: &[RetrievalHit], polarity: f32) {
        for hit in hits {
            self.apply(hit, polarity);
        }
    }

    fn nudge_weight(weights: &mut SoftWeights, component_key: &str, delta: f32) {
        match component_key {
            SCORE_COMPONENT_CONTENT_MATCH_KEY => {
                weights.content_match =
                    (weights.content_match + delta).clamp(MIN_WEIGHT, MAX_WEIGHT);
            }
            SCORE_COMPONENT_GOAL_RELEVANCE_KEY => {
                weights.goal_relevance =
                    (weights.goal_relevance + delta).clamp(MIN_WEIGHT, MAX_WEIGHT);
            }
            SCORE_COMPONENT_SELF_RELEVANCE_KEY => {
                weights.self_relevance =
                    (weights.self_relevance + delta).clamp(MIN_WEIGHT, MAX_WEIGHT);
            }
            SCORE_COMPONENT_SALIENCE_KEY => {
                weights.salience = (weights.salience + delta).clamp(MIN_WEIGHT, MAX_WEIGHT);
            }
            SCORE_COMPONENT_EVIDENCE_STRENGTH_KEY => {
                weights.evidence_strength =
                    (weights.evidence_strength + delta).clamp(MIN_WEIGHT, MAX_WEIGHT);
            }
            SCORE_COMPONENT_RECENCY_KEY => {
                weights.recency = (weights.recency + delta).clamp(MIN_WEIGHT, MAX_WEIGHT);
            }
            SCORE_COMPONENT_PROCEDURE_SUCCESS_KEY => {
                weights.procedure_success =
                    (weights.procedure_success + delta).clamp(MIN_WEIGHT, MAX_WEIGHT);
            }
            _ => {}
        }
    }
}

impl FeedbackAdjuster {
    /// Convert signals from a [`MemoryFeedbackStore`] into polarity values and
    /// apply them to this adjuster.
    ///
    /// Signal → polarity mapping:
    /// - [`Helpful`]        → `+1.0`
    /// - [`Promote`]        → `+1.5`
    /// - [`Wrong`]          → `-1.0`
    /// - [`CausedBadOutput`]→ `-1.0`
    /// - [`Irrelevant`]     → `-0.5`
    /// - [`Suppress`]       → `-0.5`
    ///
    /// Only hits whose `memory_id` has a recorded signal are processed; others
    /// are silently skipped.
    ///
    /// [`Helpful`]: crate::agent_memory::memory_feedback::FeedbackSignal::Helpful
    /// [`Promote`]: crate::agent_memory::memory_feedback::FeedbackSignal::Promote
    /// [`Wrong`]: crate::agent_memory::memory_feedback::FeedbackSignal::Wrong
    /// [`CausedBadOutput`]: crate::agent_memory::memory_feedback::FeedbackSignal::CausedBadOutput
    /// [`Irrelevant`]: crate::agent_memory::memory_feedback::FeedbackSignal::Irrelevant
    /// [`Suppress`]: crate::agent_memory::memory_feedback::FeedbackSignal::Suppress
    pub fn apply_from_store(
        &mut self,
        store: &super::memory_feedback::MemoryFeedbackStore,
        hits: &[RetrievalHit],
    ) {
        use super::memory_feedback::FeedbackSignal;
        for hit in hits {
            let memory_id = match hit.memory_id.as_deref() {
                Some(id) => id,
                None => continue,
            };
            let polarity = match store.signal_for(memory_id) {
                Some(FeedbackSignal::Helpful) => 1.0_f32,
                Some(FeedbackSignal::Promote) => 1.5_f32,
                Some(FeedbackSignal::Wrong | FeedbackSignal::CausedBadOutput) => -1.0_f32,
                Some(FeedbackSignal::Irrelevant | FeedbackSignal::Suppress) => -0.5_f32,
                None => continue,
            };
            self.apply(hit, polarity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_memory::policy::PolicyProfile;
    use crate::agent_memory::schemas::RetrievalHit;
    use chrono::Utc;
    use std::collections::BTreeMap;

    fn baseline() -> SoftWeights {
        PolicyProfile::default().soft_weights().clone()
    }

    /// Build a minimal scored `RetrievalHit` with pre-populated component
    /// metadata as the ranker would produce.
    fn scored_hit(content_match_contribution: f32, total: f32) -> RetrievalHit {
        let mut metadata = BTreeMap::new();
        metadata.insert(
            SCORE_COMPONENT_CONTENT_MATCH_KEY.to_string(),
            format!("{content_match_contribution:.6}"),
        );
        metadata.insert(SCORE_COMPONENT_TOTAL_KEY.to_string(), format!("{total:.6}"));
        RetrievalHit {
            memory_id: None,
            belief_id: None,
            entity: None,
            slot: None,
            value: None,
            text: String::new(),
            memory_layer: None,
            memory_type: None,
            score: total,
            timestamp: Utc::now(),
            scope: None,
            source: None,
            from_belief: false,
            expired: false,
            metadata,
        }
    }

    #[test]
    fn positive_feedback_raises_dominant_content_match_weight() {
        let initial = baseline().content_match;
        let mut adjuster = FeedbackAdjuster::new(baseline());
        // content_match contributes 2.0 out of 4.0 total = 50% → dominant
        adjuster.apply(&scored_hit(2.0, 4.0), 1.0);
        assert!(
            adjuster.weights().content_match > initial,
            "expected weight to increase: got {}",
            adjuster.weights().content_match
        );
    }

    #[test]
    fn negative_feedback_lowers_dominant_content_match_weight() {
        let initial = baseline().content_match;
        let mut adjuster = FeedbackAdjuster::new(baseline());
        adjuster.apply(&scored_hit(2.0, 4.0), -1.0);
        assert!(
            adjuster.weights().content_match < initial,
            "expected weight to decrease: got {}",
            adjuster.weights().content_match
        );
    }

    #[test]
    fn non_dominant_component_is_not_nudged() {
        let initial = baseline().content_match;
        let mut adjuster = FeedbackAdjuster::new(baseline());
        // content_match contributes 0.1 out of 10.0 total = 1% → below threshold
        adjuster.apply(&scored_hit(0.1, 10.0), 1.0);
        assert_eq!(
            adjuster.weights().content_match,
            initial,
            "non-dominant component weight should not change"
        );
    }

    #[test]
    fn weight_never_exceeds_maximum() {
        let mut adjuster = FeedbackAdjuster::with_learning_rate(baseline(), 0.5);
        for _ in 0..100 {
            adjuster.apply(&scored_hit(5.0, 8.0), 1.0);
        }
        assert_eq!(
            adjuster.weights().content_match,
            MAX_WEIGHT,
            "weight must be clamped to MAX_WEIGHT"
        );
    }

    #[test]
    fn weight_never_falls_below_minimum() {
        let mut adjuster = FeedbackAdjuster::with_learning_rate(baseline(), 0.5);
        for _ in 0..100 {
            adjuster.apply(&scored_hit(5.0, 8.0), -1.0);
        }
        assert_eq!(
            adjuster.weights().content_match,
            MIN_WEIGHT,
            "weight must be clamped to MIN_WEIGHT"
        );
    }

    #[test]
    fn feedback_count_increments_on_each_valid_signal() {
        let mut adjuster = FeedbackAdjuster::new(baseline());
        adjuster.apply(&scored_hit(2.0, 4.0), 1.0);
        adjuster.apply(&scored_hit(2.0, 4.0), -1.0);
        adjuster.apply(&scored_hit(2.0, 4.0), 1.0);
        assert_eq!(adjuster.feedback_count(), 3);
    }

    #[test]
    fn no_op_when_hit_has_no_scored_metadata() {
        let initial = baseline();
        let mut adjuster = FeedbackAdjuster::new(initial.clone());
        let empty_hit = scored_hit(0.0, 0.0); // total_abs < EPSILON
        adjuster.apply(&empty_hit, 1.0);
        assert_eq!(
            adjuster.feedback_count(),
            0,
            "no-op hit must not increment count"
        );
        assert_eq!(
            adjuster.weights().content_match,
            initial.content_match,
            "weights must be unchanged after no-op"
        );
    }

    #[test]
    fn apply_all_processes_each_hit_in_order() {
        let mut adjuster = FeedbackAdjuster::new(baseline());
        let hits = vec![scored_hit(2.0, 4.0), scored_hit(2.0, 4.0)];
        adjuster.apply_all(&hits, 1.0);
        assert_eq!(adjuster.feedback_count(), 2);
    }
}
