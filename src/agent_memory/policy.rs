pub mod policy_profile;
pub mod reason_codes;

pub use policy_profile::{HardConstraints, PolicyProfile, SoftWeights};
pub use reason_codes::ReasonCode;

use super::enums::{MemoryLayer, MemoryType, SelfModelKind, SourceType};
use super::schemas::RetentionRule;

const DAY_SECONDS: i64 = 86_400;

/// Deterministic default policy values for governed memory.
#[derive(Debug, Clone)]
pub struct PolicySet {
    reject: f32,
    trace_only: f32,
    promote: f32,
    belief_promote: f32,
    self_model_promote: f32,
    goal_state_promote: f32,
    procedure_promote: f32,
    self_model_repetitions: usize,
    procedure_success_repetitions: usize,
    blocker_repetitions: usize,
    belief_stabilization_repetitions: usize,
    consolidation_window_days: i64,
    belief_stability_min_days: i64,
    trusted_belief_source_weight: f32,
    trusted_self_model_source_weight: f32,
    persist_retrieval_touches: bool,
}

impl Default for PolicySet {
    fn default() -> Self {
        Self {
            reject: 0.25,
            trace_only: 0.35,
            promote: 0.65,
            belief_promote: 0.75,
            self_model_promote: 0.70,
            goal_state_promote: 0.65,
            procedure_promote: 0.72,
            self_model_repetitions: 2,
            procedure_success_repetitions: 2,
            blocker_repetitions: 3,
            belief_stabilization_repetitions: 2,
            consolidation_window_days: 30,
            belief_stability_min_days: 3,
            trusted_belief_source_weight: 0.8,
            trusted_self_model_source_weight: 0.9,
            persist_retrieval_touches: true,
        }
    }
}

impl PolicySet {
    #[must_use]
    pub fn promotion_score(confidence: f32, salience: f32) -> f32 {
        (confidence.clamp(0.0, 1.0) * 0.6) + (salience.clamp(0.0, 1.0) * 0.4)
    }

    #[must_use]
    pub fn reject_threshold(&self) -> f32 {
        self.reject
    }

    #[must_use]
    pub fn store_trace_threshold(&self) -> f32 {
        self.trace_only
    }

    #[must_use]
    pub fn promote_threshold(&self, memory_layer: MemoryLayer) -> f32 {
        match memory_layer {
            MemoryLayer::Belief => self.belief_promote,
            MemoryLayer::SelfModel => self.self_model_promote,
            MemoryLayer::GoalState => self.goal_state_promote,
            MemoryLayer::Episode => self.promote,
            MemoryLayer::Procedure => self.procedure_promote,
            MemoryLayer::Trace => 1.1,
        }
    }

    #[must_use]
    pub fn minimum_self_model_repetitions(&self) -> usize {
        self.self_model_repetitions
    }

    #[must_use]
    pub fn minimum_procedure_success_repetitions(&self) -> usize {
        self.procedure_success_repetitions
    }

    #[must_use]
    pub fn minimum_blocker_repetitions(&self) -> usize {
        self.blocker_repetitions
    }

    #[must_use]
    pub fn minimum_belief_stabilization_repetitions(&self) -> usize {
        self.belief_stabilization_repetitions
    }

    #[must_use]
    pub fn consolidation_window_days(&self) -> i64 {
        self.consolidation_window_days
    }

    #[must_use]
    pub fn belief_stability_min_days(&self) -> i64 {
        self.belief_stability_min_days
    }

    #[must_use]
    pub fn trusted_belief_source_weight(&self) -> f32 {
        self.trusted_belief_source_weight
    }

    #[must_use]
    pub fn trusted_self_model_source_weight(&self) -> f32 {
        self.trusted_self_model_source_weight
    }

    #[must_use]
    pub fn persist_retrieval_touches(&self) -> bool {
        self.persist_retrieval_touches
    }

    #[must_use]
    pub fn with_persist_retrieval_touches(mut self, enabled: bool) -> Self {
        self.persist_retrieval_touches = enabled;
        self
    }

    #[must_use]
    pub fn policy_profile(&self) -> PolicyProfile {
        PolicyProfile::from_policy_set(self)
    }

    #[must_use]
    pub fn allows_singleton_self_model_kind(&self, kind: SelfModelKind) -> bool {
        matches!(
            kind,
            SelfModelKind::Preference
                | SelfModelKind::ResponseStyle
                | SelfModelKind::ToolPreference
                | SelfModelKind::ProjectNorm
                | SelfModelKind::Constraint
                | SelfModelKind::Value
                | SelfModelKind::CapabilityLimit
                | SelfModelKind::WorkPattern
        )
    }

    #[must_use]
    pub fn allows_singleton_belief_from_trusted_source(
        &self,
        source_type: SourceType,
        trust_weight: f32,
    ) -> bool {
        matches!(source_type, SourceType::System | SourceType::Tool)
            && trust_weight >= self.trusted_belief_source_weight
    }

    #[must_use]
    pub fn allows_singleton_self_model_from_trusted_source(
        &self,
        source_type: SourceType,
        trust_weight: f32,
    ) -> bool {
        matches!(source_type, SourceType::System | SourceType::Tool)
            && trust_weight >= self.trusted_self_model_source_weight
    }

    #[must_use]
    pub fn retention_rule(
        &self,
        memory_layer: MemoryLayer,
        memory_type: MemoryType,
    ) -> RetentionRule {
        match memory_layer {
            MemoryLayer::Trace => RetentionRule {
                memory_layer,
                memory_type,
                default_ttl: Some(3 * DAY_SECONDS),
                decay_per_day: 0.18,
                retrieval_priority: 0.1,
                promotable: false,
            },
            MemoryLayer::Episode => RetentionRule {
                memory_layer,
                memory_type,
                default_ttl: Some(30 * DAY_SECONDS),
                decay_per_day: 0.04,
                retrieval_priority: 0.45,
                promotable: true,
            },
            MemoryLayer::Belief => RetentionRule {
                memory_layer,
                memory_type,
                default_ttl: None,
                decay_per_day: 0.005,
                retrieval_priority: 0.75,
                promotable: true,
            },
            MemoryLayer::SelfModel => RetentionRule {
                memory_layer,
                memory_type,
                default_ttl: None,
                decay_per_day: 0.002,
                retrieval_priority: 1.0,
                promotable: true,
            },
            MemoryLayer::GoalState => RetentionRule {
                memory_layer,
                memory_type,
                default_ttl: Some(14 * DAY_SECONDS),
                decay_per_day: 0.03,
                retrieval_priority: 0.95,
                promotable: true,
            },
            MemoryLayer::Procedure => RetentionRule {
                memory_layer,
                memory_type,
                default_ttl: Some(90 * DAY_SECONDS),
                decay_per_day: 0.01,
                retrieval_priority: 0.7,
                promotable: true,
            },
        }
    }
}
