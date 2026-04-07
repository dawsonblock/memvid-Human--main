use std::collections::BTreeMap;

use uuid::Uuid;

use super::clock::Clock;
use super::enums::{MemoryLayer, PromotionDecision, SelfModelKind};
use super::policy::PolicySet;
use super::schemas::{CandidateMemory, DurableMemory, PromotionContext, PromotionResult};

#[derive(Debug, Clone)]
struct DestinationEligibility {
    allowed: bool,
    reason: String,
    route_basis: &'static str,
    fallback_layer: &'static str,
}

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
    pub fn policy(&self) -> &PolicySet {
        &self.policy
    }

    #[must_use]
    pub fn promote(&self, candidate: &CandidateMemory, clock: &dyn Clock) -> PromotionResult {
        self.promote_with_context(candidate, &PromotionContext::default(), clock)
    }

    #[must_use]
    pub fn promote_with_context(
        &self,
        candidate: &CandidateMemory,
        context: &PromotionContext,
        clock: &dyn Clock,
    ) -> PromotionResult {
        let score = PolicySet::promotion_score(candidate.confidence, candidate.salience);
        let layer = candidate.memory_layer();
        let mut details = self.base_details(candidate, context, score);

        if score < self.policy.reject_threshold() {
            details.insert("route_basis".to_string(), "rejected".to_string());
            details.insert("fallback_layer".to_string(), "trace".to_string());
            return PromotionResult {
                decision: PromotionDecision::Reject,
                score,
                reason: "score below rejection threshold".to_string(),
                durable_memory: None,
                details,
            };
        }

        if layer == MemoryLayer::Trace || score < self.policy.store_trace_threshold() {
            details.insert("route_basis".to_string(), "insufficient_score".to_string());
            details.insert("fallback_layer".to_string(), "trace".to_string());
            return PromotionResult {
                decision: PromotionDecision::StoreTrace,
                score,
                reason: "candidate retained as trace only".to_string(),
                durable_memory: None,
                details,
            };
        }

        let eligibility = self.destination_eligibility(candidate, context);
        details.insert(
            "route_basis".to_string(),
            eligibility.route_basis.to_string(),
        );
        details.insert(
            "fallback_layer".to_string(),
            eligibility.fallback_layer.to_string(),
        );

        if !eligibility.allowed {
            return PromotionResult {
                decision: PromotionDecision::StoreTrace,
                score,
                reason: eligibility.reason,
                durable_memory: None,
                details,
            };
        }

        if score < self.policy.promote_threshold(layer) {
            return PromotionResult {
                decision: PromotionDecision::StoreTrace,
                score,
                reason: "candidate did not meet promotion threshold".to_string(),
                durable_memory: None,
                details,
            };
        }

        let promotion_route = details
            .get("route_basis")
            .cloned()
            .unwrap_or_else(|| "singleton".to_string());
        let evidence_count = self.evidence_count(layer, context).to_string();
        let mut metadata = candidate.metadata.clone();
        metadata.insert("promotion_route_basis".to_string(), promotion_route);
        metadata.insert("promotion_evidence_count".to_string(), evidence_count);
        metadata.insert(
            "promotion_verified_source".to_string(),
            context.verified_source.to_string(),
        );
        metadata.insert(
            "promotion_seeded_by_system".to_string(),
            context.seeded_by_system.to_string(),
        );

        PromotionResult {
            decision: PromotionDecision::Promote,
            score,
            reason: "candidate promoted to durable memory".to_string(),
            durable_memory: Some(DurableMemory {
                memory_id: Uuid::new_v4().to_string(),
                candidate_id: candidate.candidate_id.clone(),
                stored_at: clock.now(),
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
                metadata,
                is_retraction: candidate.is_retraction,
            }),
            details,
        }
    }

    fn base_details(
        &self,
        candidate: &CandidateMemory,
        context: &PromotionContext,
        score: f32,
    ) -> BTreeMap<String, String> {
        let layer = candidate.memory_layer();
        BTreeMap::from([
            ("target_layer".to_string(), layer.as_str().to_string()),
            (
                "score_threshold".to_string(),
                self.policy.promote_threshold(layer).to_string(),
            ),
            (
                "source_type".to_string(),
                format!("{:?}", candidate.source.source_type).to_lowercase(),
            ),
            (
                "source_trust_weight".to_string(),
                candidate.source.trust_weight.to_string(),
            ),
            ("score".to_string(), score.to_string()),
            (
                "evidence_count".to_string(),
                self.evidence_count(layer, context).to_string(),
            ),
            (
                "verified_source".to_string(),
                context.verified_source.to_string(),
            ),
            (
                "seeded_by_system".to_string(),
                context.seeded_by_system.to_string(),
            ),
        ])
    }

    fn destination_eligibility(
        &self,
        candidate: &CandidateMemory,
        context: &PromotionContext,
    ) -> DestinationEligibility {
        match candidate.memory_layer() {
            MemoryLayer::Trace => DestinationEligibility {
                allowed: false,
                reason: "trace observations are archival only".to_string(),
                route_basis: "trace_only",
                fallback_layer: "trace",
            },
            MemoryLayer::Episode => self.can_promote_to_episode(candidate),
            MemoryLayer::GoalState => self.can_promote_to_goal_state(candidate),
            MemoryLayer::Belief => self.can_promote_to_belief(candidate, context),
            MemoryLayer::SelfModel => self.can_promote_to_self_model(candidate, context),
            MemoryLayer::Procedure => self.can_promote_to_procedure(candidate, context),
        }
    }

    fn can_promote_to_episode(&self, candidate: &CandidateMemory) -> DestinationEligibility {
        if !Self::is_event_like(candidate) {
            return DestinationEligibility {
                allowed: false,
                reason: "episode promotion requires event-like evidence".to_string(),
                route_basis: "insufficient_semantics",
                fallback_layer: "trace",
            };
        }

        DestinationEligibility {
            allowed: true,
            reason: "episode promotion allowed for event-like observation".to_string(),
            route_basis: "singleton",
            fallback_layer: "episode",
        }
    }

    fn can_promote_to_goal_state(&self, candidate: &CandidateMemory) -> DestinationEligibility {
        if candidate.slot.is_none() || candidate.value.is_none() {
            return DestinationEligibility {
                allowed: false,
                reason: "goal-state promotion requires structured slot and value".to_string(),
                route_basis: "insufficient_structure",
                fallback_layer: if Self::is_event_like(candidate) {
                    "episode"
                } else {
                    "trace"
                },
            };
        }
        if !Self::has_goal_state_semantics(candidate) {
            return DestinationEligibility {
                allowed: false,
                reason: "goal-state promotion requires explicit active task or blocker semantics"
                    .to_string(),
                route_basis: "insufficient_semantics",
                fallback_layer: if Self::is_event_like(candidate) {
                    "episode"
                } else {
                    "trace"
                },
            };
        }

        DestinationEligibility {
            allowed: true,
            reason: "goal-state promotion allowed for explicit task-state observation".to_string(),
            route_basis: "singleton",
            fallback_layer: "episode",
        }
    }

    fn can_promote_to_belief(
        &self,
        candidate: &CandidateMemory,
        context: &PromotionContext,
    ) -> DestinationEligibility {
        if candidate.entity.is_none() || candidate.slot.is_none() || candidate.value.is_none() {
            return DestinationEligibility {
                allowed: false,
                reason: "belief promotion requires entity, slot, and value".to_string(),
                route_basis: "insufficient_structure",
                fallback_layer: if Self::should_preserve_as_episode(candidate) {
                    "episode"
                } else {
                    "trace"
                },
            };
        }
        if context.verified_source {
            return DestinationEligibility {
                allowed: true,
                reason: "belief promotion allowed for verified source evidence".to_string(),
                route_basis: "verified_source",
                fallback_layer: "episode",
            };
        }
        if context.belief_evidence_count >= self.policy.minimum_belief_stabilization_repetitions() {
            return DestinationEligibility {
                allowed: true,
                reason: "belief promotion allowed after repeated matching evidence".to_string(),
                route_basis: "repeated_evidence",
                fallback_layer: "episode",
            };
        }
        if self.policy.allows_singleton_belief_from_trusted_source(
            candidate.source.source_type,
            candidate.source.trust_weight,
        ) {
            return DestinationEligibility {
                allowed: true,
                reason: "belief promotion allowed for trusted source evidence".to_string(),
                route_basis: "trusted_source",
                fallback_layer: "episode",
            };
        }

        DestinationEligibility {
            allowed: false,
            reason: "belief promotion requires repeated evidence, verified source, or trusted source"
                .to_string(),
            route_basis: "insufficient_evidence",
            fallback_layer: "episode",
        }
    }

    fn can_promote_to_self_model(
        &self,
        candidate: &CandidateMemory,
        context: &PromotionContext,
    ) -> DestinationEligibility {
        if candidate.entity.is_none() || candidate.slot.is_none() || candidate.value.is_none() {
            return DestinationEligibility {
                allowed: false,
                reason: "self-model promotion requires entity, slot, and value".to_string(),
                route_basis: "insufficient_structure",
                fallback_layer: if Self::should_preserve_as_episode(candidate) {
                    "episode"
                } else {
                    "trace"
                },
            };
        }
        if context.self_model_evidence_count >= self.policy.minimum_self_model_repetitions() {
            return DestinationEligibility {
                allowed: true,
                reason: "self-model promotion allowed after repeated stable evidence".to_string(),
                route_basis: "repeated_evidence",
                fallback_layer: "episode",
            };
        }
        if Self::is_explicit_durable_self_model_statement(candidate)
            && self.policy.allows_singleton_self_model_from_trusted_source(
                candidate.source.source_type,
                candidate.source.trust_weight,
            )
        {
            return DestinationEligibility {
                allowed: true,
                reason: "self-model promotion allowed for explicit durable trusted statement"
                    .to_string(),
                route_basis: "trusted_source",
                fallback_layer: "episode",
            };
        }
        if context.verified_source && Self::is_explicit_durable_self_model_statement(candidate) {
            return DestinationEligibility {
                allowed: true,
                reason: "self-model promotion allowed for verified durable statement".to_string(),
                route_basis: "verified_source",
                fallback_layer: "episode",
            };
        }

        DestinationEligibility {
            allowed: false,
            reason: "self-model promotion requires repeated evidence or an explicit durable trusted statement"
                .to_string(),
            route_basis: "insufficient_evidence",
            fallback_layer: "episode",
        }
    }

    fn can_promote_to_procedure(
        &self,
        candidate: &CandidateMemory,
        context: &PromotionContext,
    ) -> DestinationEligibility {
        if Self::workflow_key(candidate).is_none() {
            return DestinationEligibility {
                allowed: false,
                reason: "procedure promotion requires a workflow key".to_string(),
                route_basis: "insufficient_structure",
                fallback_layer: if Self::is_event_like(candidate) {
                    "episode"
                } else {
                    "trace"
                },
            };
        }
        if context.seeded_by_system {
            return DestinationEligibility {
                allowed: true,
                reason: "procedure promotion allowed for explicitly seeded system workflow"
                    .to_string(),
                route_basis: "system_seeded",
                fallback_layer: "episode",
            };
        }
        if context.procedure_success_count >= self.policy.minimum_procedure_success_repetitions() {
            return DestinationEligibility {
                allowed: true,
                reason: "procedure promotion allowed after repeated successful workflow evidence"
                    .to_string(),
                route_basis: "repeated_evidence",
                fallback_layer: "episode",
            };
        }

        DestinationEligibility {
            allowed: false,
            reason: "procedure promotion requires repeated successful evidence or explicit system seeding"
                .to_string(),
            route_basis: "insufficient_evidence",
            fallback_layer: if Self::is_event_like(candidate) {
                "episode"
            } else {
                "trace"
            },
        }
    }

    fn evidence_count(&self, layer: MemoryLayer, context: &PromotionContext) -> usize {
        match layer {
            MemoryLayer::Belief => context.belief_evidence_count,
            MemoryLayer::SelfModel => context.self_model_evidence_count,
            MemoryLayer::GoalState => context.goal_state_evidence_count,
            MemoryLayer::Procedure => context.procedure_success_count,
            MemoryLayer::Episode => 1,
            MemoryLayer::Trace => 0,
        }
    }

    fn should_preserve_as_episode(candidate: &CandidateMemory) -> bool {
        candidate.memory_layer() != MemoryLayer::Trace
            && (candidate.entity.is_some()
                || candidate.slot.is_some()
                || candidate.value.is_some()
                || Self::is_event_like(candidate)
                || Self::workflow_key(candidate).is_some())
    }

    fn is_event_like(candidate: &CandidateMemory) -> bool {
        if candidate.memory_layer() == MemoryLayer::Episode || candidate.event_at.is_some() {
            return true;
        }

        let lower = candidate.raw_text.to_lowercase();
        [
            "completed",
            "failed",
            "started",
            "happened",
            "yesterday",
            "today",
            "workflow",
        ]
        .iter()
        .filter(|marker| lower.contains(**marker))
        .count()
            >= 2
            || candidate.metadata.contains_key("outcome")
            || candidate.metadata.contains_key("workflow_key")
    }

    fn has_goal_state_semantics(candidate: &CandidateMemory) -> bool {
        let slot = candidate.slot.as_deref().unwrap_or("").to_lowercase();
        let value = candidate.value.as_deref().unwrap_or("").to_lowercase();
        let raw = candidate.raw_text.to_lowercase();
        let goal_terms = [
            "task",
            "status",
            "goal",
            "blocked",
            "waiting",
            "next",
            "todo",
            "milestone",
            "complete",
            "done",
            "active",
        ];

        goal_terms.iter().any(|term| slot.contains(term))
            || goal_terms.iter().any(|term| value.contains(term))
            || goal_terms.iter().any(|term| raw.contains(term))
    }

    fn is_explicit_durable_self_model_statement(candidate: &CandidateMemory) -> bool {
        let slot = candidate.slot.as_deref().unwrap_or("");
        let lower = format!(
            "{} {}",
            candidate.value.as_deref().unwrap_or(""),
            candidate.raw_text
        )
        .to_lowercase();
        let durable_language = [
            "prefer",
            "preference",
            "constraint",
            "always",
            "never",
            "avoid",
            "style",
        ];

        matches!(
            SelfModelKind::from_slot(slot),
            SelfModelKind::Preference
                | SelfModelKind::ResponseStyle
                | SelfModelKind::ToolPreference
                | SelfModelKind::Constraint
                | SelfModelKind::WorkPattern
                | SelfModelKind::ProjectNorm
        ) && durable_language.iter().any(|term| lower.contains(term))
    }

    fn workflow_key(candidate: &CandidateMemory) -> Option<&str> {
        candidate
            .metadata
            .get("workflow_key")
            .map(String::as_str)
            .or(candidate.slot.as_deref())
    }
}