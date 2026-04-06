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

        PromotionResult {
            decision: PromotionDecision::Promote,
            score,
            reason: "candidate promoted to durable memory".to_string(),
            durable_memory: Some(DurableMemory {
                memory_id: Uuid::new_v4().to_string(),
                candidate_id: candidate.candidate_id.clone(),
                stored_at: clock.now(),
                entity: candidate.entity.clone(),
                slot: candidate.slot.clone(),
                value: candidate.value.clone(),
                raw_text: candidate.raw_text.clone(),
                memory_type: candidate.memory_type,
                confidence: candidate.confidence,
                salience: candidate.salience,
                scope: candidate.scope,
                ttl: candidate.ttl.or(
                    self.policy
                        .retention_rule(candidate.memory_layer(), candidate.memory_type)
                        .default_ttl,
                ),
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
