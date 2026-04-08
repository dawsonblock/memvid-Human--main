use serde::{Deserialize, Serialize};

use super::super::enums::{MemoryLayer, SourceType};
use super::PolicySet;

/// Protected governance profile layered on top of the compatibility-first `PolicySet`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyProfile {
    version: u16,
    constraints: HardConstraints,
    weights: SoftWeights,
}

/// Hard fail-closed rules that gate whether a candidate may affect durable memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HardConstraints {
    pub reject_threshold: f32,
    pub trace_only_threshold: f32,
    pub episode_promotion_threshold: f32,
    pub belief_promotion_threshold: f32,
    pub self_model_promotion_threshold: f32,
    pub goal_state_promotion_threshold: f32,
    pub procedure_promotion_threshold: f32,
    pub minimum_belief_evidence: usize,
    pub minimum_self_model_evidence: usize,
    pub minimum_procedure_success_evidence: usize,
    pub minimum_blocker_evidence: usize,
    pub require_non_empty_structured_identity: bool,
    pub require_goal_state_semantics: bool,
    pub prohibit_untrusted_singleton_belief_promotion: bool,
    pub protect_self_model_identity: bool,
    pub require_supported_singleton_self_model_kind: bool,
    pub require_explicit_self_model_statement: bool,
    pub stable_directive_requires_trusted_update_path: bool,
    pub minimum_stable_directive_update_evidence: usize,
    pub require_system_seed_or_repeated_procedure_evidence: bool,
    pub trusted_belief_source_weight: f32,
    pub trusted_self_model_source_weight: f32,
}

/// Tunable weights used by explainable ranking and promotion scoring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoftWeights {
    pub promotion_confidence: f32,
    pub promotion_salience: f32,
    pub content_match: f32,
    pub goal_relevance: f32,
    pub self_relevance: f32,
    pub salience: f32,
    pub evidence_strength: f32,
    pub contradiction_penalty: f32,
    pub recency: f32,
    pub procedure_success: f32,
}

impl PolicyProfile {
    #[must_use]
    pub fn from_policy_set(policy: &PolicySet) -> Self {
        Self {
            version: 1,
            constraints: HardConstraints {
                reject_threshold: policy.reject_threshold(),
                trace_only_threshold: policy.store_trace_threshold(),
                episode_promotion_threshold: policy.promote_threshold(MemoryLayer::Episode),
                belief_promotion_threshold: policy.promote_threshold(MemoryLayer::Belief),
                self_model_promotion_threshold: policy.promote_threshold(MemoryLayer::SelfModel),
                goal_state_promotion_threshold: policy.promote_threshold(MemoryLayer::GoalState),
                procedure_promotion_threshold: policy.promote_threshold(MemoryLayer::Procedure),
                minimum_belief_evidence: policy.minimum_belief_stabilization_repetitions(),
                minimum_self_model_evidence: policy.minimum_self_model_repetitions(),
                minimum_procedure_success_evidence: policy.minimum_procedure_success_repetitions(),
                minimum_blocker_evidence: policy.minimum_blocker_repetitions(),
                require_non_empty_structured_identity: true,
                require_goal_state_semantics: true,
                prohibit_untrusted_singleton_belief_promotion: true,
                protect_self_model_identity: true,
                require_supported_singleton_self_model_kind: true,
                require_explicit_self_model_statement: true,
                stable_directive_requires_trusted_update_path: true,
                minimum_stable_directive_update_evidence: policy.minimum_self_model_repetitions(),
                require_system_seed_or_repeated_procedure_evidence: true,
                trusted_belief_source_weight: policy.trusted_belief_source_weight(),
                trusted_self_model_source_weight: policy.trusted_self_model_source_weight(),
            },
            weights: SoftWeights {
                promotion_confidence: 0.6,
                promotion_salience: 0.4,
                content_match: 0.3,
                goal_relevance: 0.2,
                self_relevance: 0.2,
                salience: 0.15,
                evidence_strength: 0.25,
                contradiction_penalty: 0.2,
                recency: 0.15,
                procedure_success: 0.2,
            },
        }
    }

    #[must_use]
    pub const fn version(&self) -> u16 {
        self.version
    }

    #[must_use]
    pub const fn hard_constraints(&self) -> &HardConstraints {
        &self.constraints
    }

    #[must_use]
    pub const fn soft_weights(&self) -> &SoftWeights {
        &self.weights
    }

    #[must_use]
    pub fn promotion_score(&self, confidence: f32, salience: f32) -> f32 {
        (confidence.clamp(0.0, 1.0) * self.weights.promotion_confidence)
            + (salience.clamp(0.0, 1.0) * self.weights.promotion_salience)
    }

    #[must_use]
    pub const fn reject_threshold(&self) -> f32 {
        self.constraints.reject_threshold
    }

    #[must_use]
    pub const fn store_trace_threshold(&self) -> f32 {
        self.constraints.trace_only_threshold
    }

    #[must_use]
    pub const fn promote_threshold(&self, memory_layer: MemoryLayer) -> f32 {
        match memory_layer {
            MemoryLayer::Episode => self.constraints.episode_promotion_threshold,
            MemoryLayer::Belief => self.constraints.belief_promotion_threshold,
            MemoryLayer::SelfModel => self.constraints.self_model_promotion_threshold,
            MemoryLayer::GoalState => self.constraints.goal_state_promotion_threshold,
            MemoryLayer::Procedure => self.constraints.procedure_promotion_threshold,
            MemoryLayer::Trace => 1.1,
        }
    }

    #[must_use]
    pub const fn minimum_belief_evidence(&self) -> usize {
        self.constraints.minimum_belief_evidence
    }

    #[must_use]
    pub const fn minimum_self_model_evidence(&self) -> usize {
        self.constraints.minimum_self_model_evidence
    }

    #[must_use]
    pub const fn stable_directive_requires_trusted_update_path(&self) -> bool {
        self.constraints
            .stable_directive_requires_trusted_update_path
    }

    #[must_use]
    pub const fn minimum_stable_directive_update_evidence(&self) -> usize {
        self.constraints.minimum_stable_directive_update_evidence
    }

    #[must_use]
    pub const fn minimum_procedure_success_evidence(&self) -> usize {
        self.constraints.minimum_procedure_success_evidence
    }

    #[must_use]
    pub const fn minimum_blocker_evidence(&self) -> usize {
        self.constraints.minimum_blocker_evidence
    }

    #[must_use]
    pub fn allows_singleton_belief_from_trusted_source(
        &self,
        source_type: SourceType,
        trust_weight: f32,
    ) -> bool {
        self.constraints
            .prohibit_untrusted_singleton_belief_promotion
            && matches!(source_type, SourceType::System | SourceType::Tool)
            && trust_weight >= self.constraints.trusted_belief_source_weight
    }

    #[must_use]
    pub fn allows_singleton_self_model_from_trusted_source(
        &self,
        source_type: SourceType,
        trust_weight: f32,
    ) -> bool {
        self.constraints.protect_self_model_identity
            && matches!(source_type, SourceType::System | SourceType::Tool)
            && trust_weight >= self.constraints.trusted_self_model_source_weight
    }
}

impl Default for PolicyProfile {
    fn default() -> Self {
        PolicySet::default().policy_profile()
    }
}
