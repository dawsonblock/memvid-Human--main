use uuid::Uuid;

use super::clock::Clock;
use super::enums::{MemoryLayer, PromotionDecision};
use super::policy::PolicySet;
use super::schemas::{CandidateMemory, DurableMemory, PromotionResult};

/// Deterministic promotion gate.
#[derive(Debug, Clone, Default)]
pub struct MemoryPromoter {
    policy: PolicySet,
}

impl MemoryPromoter {
    #[must_use]
    pub fn new(policy: PolicySet) -> Self {
        Self { policy }
    }

    #[must_use]
    pub fn promote(&self, candidate: &CandidateMemory, clock: &dyn Clock) -> PromotionResult {
        let score = PolicySet::promotion_score(candidate.confidence, candidate.salience);

        if score < self.policy.reject_threshold() {
            return PromotionResult {
                decision: PromotionDecision::Reject,
                score,
                reason: "score below rejection threshold".to_string(),
                durable_memory: None,
            };
        }

        if candidate.memory_layer() == MemoryLayer::Trace
            || score < self.policy.store_trace_threshold()
        {
            return PromotionResult {
                decision: PromotionDecision::StoreTrace,
                score,
                reason: "candidate retained as trace only".to_string(),
                durable_memory: None,
            };
        }

        if score < self.policy.promote_threshold(candidate.memory_layer()) {
            return PromotionResult {
                decision: PromotionDecision::StoreTrace,
                score,
                reason: "candidate did not meet promotion threshold".to_string(),
                durable_memory: None,
            };
        }

        // Per-layer structural completeness gates.
        // Belief and SelfModel layers require entity + slot + value — these are the only
        // layers that form keyed lookup records. Without all three, there is nothing to key
        // on and the result would be an unqueryable or misleading durable record.
        let requires_full_structure = matches!(
            candidate.memory_layer(),
            MemoryLayer::Belief | MemoryLayer::SelfModel
        );
        if requires_full_structure
            && (candidate.entity.is_none()
                || candidate.slot.is_none()
                || candidate.value.is_none())
        {
            return PromotionResult {
                decision: PromotionDecision::StoreTrace,
                score,
                reason: "belief/self-model promotion requires entity, slot, and value".to_string(),
                durable_memory: None,
            };
        }

        // GoalState requires at minimum a slot (the goal description) to be queryable.
        if candidate.memory_layer() == MemoryLayer::GoalState && candidate.slot.is_none() {
            return PromotionResult {
                decision: PromotionDecision::StoreTrace,
                score,
                reason: "goal-state promotion requires at least a slot".to_string(),
                durable_memory: None,
            };
        }

        // Procedure promotion additionally requires source trust weight above a floor.
        // A single low-trust observation should not become a trusted procedure template.
        if candidate.memory_layer() == MemoryLayer::Procedure
            && candidate.source.trust_weight < 0.55
        {
            return PromotionResult {
                decision: PromotionDecision::StoreTrace,
                score,
                reason: "procedure promotion requires source trust weight ≥ 0.55".to_string(),
                durable_memory: None,
            };
        }

        PromotionResult {
            decision: PromotionDecision::Promote,
            score,
            reason: "candidate promoted to durable memory".to_string(),
            durable_memory: Some(DurableMemory {
                memory_id: Uuid::new_v4().to_string(),
                candidate_id: candidate.candidate_id.clone(),
                stored_at: clock.now(),
                // Use empty string rather than None for DurableMemory fields — DurableMemory
                // represents an already-verified stored record. Empty string is the honest
                // value when the candidate had no assertion for that field.
                entity: candidate.entity.clone().unwrap_or_default(),
                slot: candidate.slot.clone().unwrap_or_default(),
                value: candidate.value.clone().unwrap_or_default(),
                raw_text: candidate.raw_text.clone(),
                memory_type: candidate.memory_type,
                confidence: candidate.confidence,
                salience: candidate.salience,
                scope: candidate.scope,
                ttl: candidate.ttl.or(self
                    .policy
                    .retention_rule(candidate.memory_layer(), candidate.memory_type)
                    .default_ttl),
                source: candidate.source.clone(),
                event_at: candidate.event_at,
                valid_from: candidate.valid_from,
                valid_to: candidate.valid_to,
                internal_layer: candidate.internal_layer,
                tags: candidate.tags.clone(),
                metadata: candidate.metadata.clone(),
                is_retraction: candidate.is_retraction,
            }),
        }
    }
}
