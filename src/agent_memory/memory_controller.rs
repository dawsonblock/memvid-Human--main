use std::collections::BTreeMap;
use std::sync::Arc;

use super::adapters::memvid_store::MemoryStore;
use super::audit::AuditLogger;
use super::belief_store::BeliefStore;
use super::belief_updater::{BeliefUpdateOutcome, BeliefUpdater};
use super::clock::Clock;
use super::consolidation_engine::ConsolidationEngine;
use super::enums::{
    BeliefAction, MemoryLayer, MemoryType, PromotionDecision, SelfModelKind,
    SelfModelStabilityClass, SourceType,
};
use super::episode_store::EpisodeStore;
use super::errors::{AgentMemoryError, Result};
use super::goal_state_store::GoalStateStore;
use super::memory_classifier::MemoryClassifier;
use super::memory_compactor::MemoryCompactor;
use super::memory_decay::MemoryDecay;
use super::memory_promoter::MemoryPromoter;
use super::memory_retriever::MemoryRetriever;
use super::policy::ReasonCode;
use super::procedure_store::{ProcedureStatusTransition, ProcedureStore};
use super::schemas::{
    AuditEvent, BeliefRecord, CandidateMemory, DurableMemory, OutcomeFeedback, PromotionContext,
    RetrievalHit, RetrievalQuery,
};
use super::self_model_store::SelfModelStore;

/// Single governed write/read authority for agent memory.
pub struct MemoryController<S: MemoryStore> {
    store: S,
    clock: Arc<dyn Clock>,
    audit: AuditLogger,
    classifier: MemoryClassifier,
    promoter: MemoryPromoter,
    belief_updater: BeliefUpdater,
    retriever: MemoryRetriever,
    consolidation_engine: ConsolidationEngine,
}

enum SelfModelGovernanceDecision {
    Persist(DurableMemory),
    Downgrade {
        reason: String,
        reason_code: ReasonCode,
    },
}

/// Explicit governed-memory maintenance result.
#[derive(Debug, Clone)]
pub struct MemoryMaintenanceReport {
    pub durable_memories: Vec<DurableMemory>,
    pub expired_ids: Vec<String>,
    pub compactor_status: &'static str,
    pub compaction_supported: bool,
    pub compactor_reason: &'static str,
}

impl<S: MemoryStore> MemoryController<S> {
    #[must_use]
    pub fn new(
        store: S,
        clock: Arc<dyn Clock>,
        audit: AuditLogger,
        classifier: MemoryClassifier,
        promoter: MemoryPromoter,
        belief_updater: BeliefUpdater,
        retriever: MemoryRetriever,
    ) -> Self {
        let consolidation_policy = promoter.policy().clone();
        Self {
            store,
            clock,
            audit,
            classifier,
            promoter,
            belief_updater,
            retriever,
            consolidation_engine: ConsolidationEngine::new(consolidation_policy),
        }
    }

    pub fn ingest(&mut self, candidate: CandidateMemory) -> Result<Option<String>> {
        // This is the only allowed path for policy-bearing memory mutations.
        let classified = self.classifier.classify(candidate);
        self.audit.emit(AuditEvent {
            event_id: String::new(),
            occurred_at: self.clock.now(),
            action: "classification".to_string(),
            candidate_id: Some(classified.candidate_id.clone()),
            memory_id: None,
            belief_id: None,
            query_text: None,
            details: {
                let mut details = BTreeMap::from([
                    (
                        "memory_type".to_string(),
                        format!("{:?}", classified.memory_type).to_lowercase(),
                    ),
                    (
                        "memory_layer".to_string(),
                        classified.memory_layer().as_str().to_string(),
                    ),
                ]);
                // Only include entity/slot in audit when they were actually asserted —
                // omitting them is the honest representation of absent structure.
                if let Some(entity) = classified.entity_non_empty() {
                    details.insert("entity".to_string(), entity.to_string());
                }
                if let Some(slot) = classified.slot_non_empty() {
                    details.insert("slot".to_string(), slot.to_string());
                }
                details
            },
        });

        let promotion_context = self.build_promotion_context(&classified)?;
        let promotion = self.promoter.promote_with_context(
            &classified,
            &promotion_context,
            self.clock.as_ref(),
        );
        let mut promotion_details = BTreeMap::from([
            (
                "decision".to_string(),
                format!("{:?}", promotion.decision).to_lowercase(),
            ),
            (
                "memory_layer".to_string(),
                classified.memory_layer().as_str().to_string(),
            ),
            ("reason".to_string(), promotion.reason.clone()),
            ("score".to_string(), promotion.score.to_string()),
        ]);
        promotion_details.extend(promotion.details.clone());
        self.audit.emit(AuditEvent {
            event_id: String::new(),
            occurred_at: self.clock.now(),
            action: "promotion".to_string(),
            candidate_id: Some(classified.candidate_id.clone()),
            memory_id: promotion
                .durable_memory
                .as_ref()
                .map(|memory| memory.memory_id.clone()),
            belief_id: None,
            query_text: None,
            details: promotion_details,
        });
        if let Some(reason_code) = promotion.reason_code {
            self.emit_policy_rejection(&classified, &promotion, reason_code);
        }

        match promotion.decision {
            PromotionDecision::Reject => Ok(None),
            PromotionDecision::StoreTrace => {
                if promotion.details.get("fallback_layer").map(String::as_str) == Some("episode") {
                    let episode_memory = {
                        let mut episode_store = EpisodeStore::new(&mut self.store);
                        episode_store.record_candidate(&classified, self.clock.as_ref())?
                    };
                    self.audit.emit(AuditEvent {
                        event_id: String::new(),
                        occurred_at: self.clock.now(),
                        action: "episode_stored".to_string(),
                        candidate_id: Some(classified.candidate_id.clone()),
                        memory_id: Some(episode_memory.memory_id.clone()),
                        belief_id: None,
                        query_text: None,
                        details: BTreeMap::from([
                            (
                                "memory_layer".to_string(),
                                MemoryLayer::Episode.as_str().to_string(),
                            ),
                            (
                                "source_layer".to_string(),
                                classified.memory_layer().as_str().to_string(),
                            ),
                            ("route_mode".to_string(), "evidence_only".to_string()),
                            (
                                "route_basis".to_string(),
                                promotion
                                    .details
                                    .get("route_basis")
                                    .cloned()
                                    .unwrap_or_else(|| "insufficient_evidence".to_string()),
                            ),
                        ]),
                    });
                    self.reconcile_procedure_statuses(Some(classified.candidate_id.clone()))?;
                    return Ok(Some(episode_memory.memory_id));
                }

                let mut trace_meta = BTreeMap::from([
                    (
                        "memory_type".to_string(),
                        format!("{:?}", MemoryType::Trace).to_lowercase(),
                    ),
                    ("memory_layer".to_string(), "trace".to_string()),
                ]);
                // Only record entity/slot when they were actually asserted.
                if let Some(entity) = classified.entity_non_empty() {
                    trace_meta.insert("entity".to_string(), entity.to_string());
                }
                if let Some(slot) = classified.slot_non_empty() {
                    trace_meta.insert("slot".to_string(), slot.to_string());
                }
                let trace_id = self.store.put_trace(&classified.raw_text, trace_meta)?;
                self.audit.emit(AuditEvent {
                    event_id: String::new(),
                    occurred_at: self.clock.now(),
                    action: "trace_stored".to_string(),
                    candidate_id: Some(classified.candidate_id.clone()),
                    memory_id: Some(trace_id.clone()),
                    belief_id: None,
                    query_text: None,
                    details: BTreeMap::new(),
                });
                self.reconcile_procedure_statuses(Some(classified.candidate_id.clone()))?;
                Ok(Some(trace_id))
            }
            PromotionDecision::Promote => {
                let Some(mut memory) = promotion.durable_memory else {
                    return Ok(None);
                };
                let memory_layer = memory.memory_layer();

                let episode_memory = if memory_layer == MemoryLayer::Episode {
                    None
                } else {
                    let episode_memory = {
                        let mut episode_store = EpisodeStore::new(&mut self.store);
                        episode_store.record_candidate(&classified, self.clock.as_ref())?
                    };
                    self.audit.emit(AuditEvent {
                        event_id: String::new(),
                        occurred_at: self.clock.now(),
                        action: "episode_stored".to_string(),
                        candidate_id: Some(classified.candidate_id.clone()),
                        memory_id: Some(episode_memory.memory_id.clone()),
                        belief_id: None,
                        query_text: None,
                        details: BTreeMap::from([
                            (
                                "memory_layer".to_string(),
                                MemoryLayer::Episode.as_str().to_string(),
                            ),
                            (
                                "source_layer".to_string(),
                                memory_layer.as_str().to_string(),
                            ),
                        ]),
                    });
                    memory = memory.with_supporting_episode(&episode_memory.memory_id);
                    Some(episode_memory)
                };

                if memory_layer == MemoryLayer::SelfModel {
                    match self.govern_self_model_memory(memory.clone())? {
                        SelfModelGovernanceDecision::Persist(governed) => {
                            memory = governed;
                        }
                        SelfModelGovernanceDecision::Downgrade {
                            reason,
                            reason_code,
                        } => {
                            self.emit_policy_rejection_event(
                                Some(classified.candidate_id.clone()),
                                MemoryLayer::SelfModel,
                                PromotionDecision::StoreTrace,
                                reason,
                                reason_code,
                                BTreeMap::from([
                                    (
                                        "route_basis".to_string(),
                                        "stable_directive_protection".to_string(),
                                    ),
                                    ("fallback_layer".to_string(), "episode".to_string()),
                                    (
                                        "policy_version".to_string(),
                                        self.promoter.policy_profile().version().to_string(),
                                    ),
                                ]),
                            );
                            self.reconcile_procedure_statuses(Some(
                                classified.candidate_id.clone(),
                            ))?;
                            return Ok(episode_memory.map(|episode| episode.memory_id));
                        }
                    }
                }

                let memory_id = self.persist_durable_memory(
                    Some(classified.candidate_id.clone()),
                    memory.clone(),
                    episode_memory
                        .as_ref()
                        .map(|episode| episode.memory_id.as_str()),
                )?;

                let episode_for_consolidation = if memory_layer == MemoryLayer::Episode {
                    Some(&memory)
                } else {
                    episode_memory.as_ref()
                };
                let primary_for_consolidation = if memory_layer == MemoryLayer::Episode {
                    None
                } else {
                    Some(&memory)
                };
                let consolidation_outcomes = self.consolidation_engine.consolidate(
                    &mut self.store,
                    episode_for_consolidation,
                    primary_for_consolidation,
                    self.clock.as_ref(),
                )?;
                for outcome in consolidation_outcomes {
                    let mut details = BTreeMap::from([
                        (
                            "target_layer".to_string(),
                            outcome.record.target_layer.as_str().to_string(),
                        ),
                        ("reason".to_string(), outcome.record.reason.clone()),
                    ]);
                    if let Some(transition) = &outcome.procedure_status_transition {
                        details.insert(
                            "previous_procedure_status".to_string(),
                            transition.previous_status.as_str().to_string(),
                        );
                        details.insert(
                            "next_procedure_status".to_string(),
                            transition.next_status.as_str().to_string(),
                        );
                    }
                    self.audit.emit(AuditEvent {
                        event_id: String::new(),
                        occurred_at: self.clock.now(),
                        action: "consolidation_recorded".to_string(),
                        candidate_id: Some(classified.candidate_id.clone()),
                        memory_id: Some(outcome.trace_id.clone()),
                        belief_id: None,
                        query_text: None,
                        details,
                    });
                    if let Some(procedure_id) = outcome.learned_procedure_id {
                        self.audit.emit(AuditEvent {
                            event_id: String::new(),
                            occurred_at: self.clock.now(),
                            action: "procedure_learned".to_string(),
                            candidate_id: Some(classified.candidate_id.clone()),
                            memory_id: Some(procedure_id),
                            belief_id: None,
                            query_text: None,
                            details: BTreeMap::from([(
                                "target_layer".to_string(),
                                MemoryLayer::Procedure.as_str().to_string(),
                            )]),
                        });
                    }
                    if let Some(transition) = outcome.procedure_status_transition {
                        let reason = Self::transition_reason_from_consolidation(&outcome.record);
                        self.emit_procedure_status_transition(
                            Some(classified.candidate_id.clone()),
                            transition,
                            "consolidation",
                            reason,
                        )?;
                    }
                }

                self.reconcile_procedure_statuses(Some(classified.candidate_id.clone()))?;

                Ok(Some(memory_id))
            }
        }
    }

    pub fn apply_durable_memory(
        &mut self,
        mut memory: DurableMemory,
        supporting_episode_id: Option<&str>,
    ) -> Result<String> {
        if memory.memory_layer() == MemoryLayer::SelfModel {
            let candidate_id = memory.candidate_id.clone();
            match self.govern_self_model_memory(memory)? {
                SelfModelGovernanceDecision::Persist(governed) => {
                    memory = governed;
                }
                SelfModelGovernanceDecision::Downgrade {
                    reason,
                    reason_code,
                } => {
                    self.emit_policy_rejection_event(
                        Some(candidate_id),
                        MemoryLayer::SelfModel,
                        PromotionDecision::Reject,
                        reason.clone(),
                        reason_code,
                        BTreeMap::from([(
                            "route_basis".to_string(),
                            "stable_directive_protection".to_string(),
                        )]),
                    );
                    return Err(AgentMemoryError::InvalidCandidate { reason });
                }
            }
        }
        let memory_id = self.persist_durable_memory(None, memory, supporting_episode_id)?;
        self.reconcile_procedure_statuses(None)?;
        Ok(memory_id)
    }

    pub fn retrieve(&mut self, query: RetrievalQuery) -> Result<Vec<RetrievalHit>> {
        let hits = self
            .retriever
            .retrieve(&mut self.store, &query, self.clock.as_ref())?;
        let touched_memory_ids = self.touch_retrieved_memories(&hits)?;
        let touch_persistence_enabled = self.retrieval_touch_persistence_enabled();
        let mut details = BTreeMap::from([
            (
                "intent".to_string(),
                format!("{:?}", query.intent).to_lowercase(),
            ),
            ("hits".to_string(), hits.len().to_string()),
            (
                "touch_persistence".to_string(),
                if touch_persistence_enabled {
                    "enabled".to_string()
                } else {
                    "disabled".to_string()
                },
            ),
        ]);
        if !touched_memory_ids.is_empty() {
            details.insert(
                "touched_memories".to_string(),
                touched_memory_ids.len().to_string(),
            );
            details.insert(
                "touched_memory_ids".to_string(),
                touched_memory_ids.join(","),
            );
        }
        self.audit.emit(AuditEvent {
            event_id: String::new(),
            occurred_at: self.clock.now(),
            action: "retrieval".to_string(),
            candidate_id: None,
            memory_id: None,
            belief_id: None,
            query_text: Some(query.query_text.clone()),
            details,
        });
        Ok(hits)
    }

    /// Convenience wrapper for obvious text-only queries.
    ///
    /// Typed `RetrievalQuery` remains the authoritative path when a caller needs exact retrieval
    /// semantics. It shares the same optional retrieval-touch side effects as `retrieve`.
    pub fn retrieve_text(&mut self, query_text: impl Into<String>) -> Result<Vec<RetrievalHit>> {
        self.retrieve(RetrievalQuery::from_text(query_text))
    }

    /// Lists the current durable memories that governed maintenance can act on.
    pub fn list_current_durable_memories(&mut self) -> Result<Vec<DurableMemory>> {
        let mut memories = Vec::new();
        for layer in [
            MemoryLayer::Episode,
            MemoryLayer::Belief,
            MemoryLayer::GoalState,
            MemoryLayer::SelfModel,
            MemoryLayer::Procedure,
        ] {
            memories.extend(self.store.list_memories_by_layer(layer)?);
        }
        memories.sort_by(|left, right| {
            left.memory_layer()
                .as_str()
                .cmp(right.memory_layer().as_str())
                .then_with(|| left.stored_at.cmp(&right.stored_at))
                .then_with(|| left.memory_id.cmp(&right.memory_id))
        });
        Ok(memories)
    }

    /// Runs the supported governed-memory maintenance flow.
    pub fn run_maintenance(&mut self) -> Result<MemoryMaintenanceReport> {
        let durable_memories = self.list_current_durable_memories()?;
        let expired_ids = MemoryDecay::from_policy(self.promoter.policy().clone()).run(
            &mut self.store,
            &durable_memories,
            self.clock.now(),
        )?;
        let compactor = MemoryCompactor;
        let compactor_reason = compactor.unsupported_reason();
        let mut details = BTreeMap::from([
            (
                "durable_memory_count".to_string(),
                durable_memories.len().to_string(),
            ),
            ("expired_count".to_string(), expired_ids.len().to_string()),
            (
                "compaction_supported".to_string(),
                compactor.is_supported().to_string(),
            ),
            (
                "compactor_status".to_string(),
                compactor.status().to_string(),
            ),
            ("compactor_reason".to_string(), compactor_reason.to_string()),
        ]);
        if !durable_memories.is_empty() {
            details.insert(
                "durable_memory_ids".to_string(),
                durable_memories
                    .iter()
                    .map(|memory| memory.memory_id.clone())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        if !expired_ids.is_empty() {
            details.insert("expired_ids".to_string(), expired_ids.join(","));
        }
        self.audit.emit(AuditEvent {
            event_id: String::new(),
            occurred_at: self.clock.now(),
            action: "maintenance".to_string(),
            candidate_id: None,
            memory_id: None,
            belief_id: None,
            query_text: None,
            details,
        });

        Ok(MemoryMaintenanceReport {
            durable_memories,
            expired_ids,
            compactor_status: compactor.status(),
            compaction_supported: compactor.is_supported(),
            compactor_reason,
        })
    }

    pub fn record_outcome_feedback(&mut self, feedback: OutcomeFeedback) -> Result<Option<String>> {
        let mut workflow_key = feedback
            .workflow_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let target_memory_id = feedback
            .memory_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let target_belief_id = feedback
            .belief_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);

        if workflow_key.is_none() && target_memory_id.is_none() && target_belief_id.is_none() {
            return Err(AgentMemoryError::InvalidCandidate {
                reason: "outcome feedback requires a memory_id, belief_id, or workflow_key"
                    .to_string(),
            });
        }

        let mut stored_memory_ids = Vec::new();
        let mut target_layer = None;

        if let Some(memory_id) = target_memory_id.as_deref() {
            if let Some(memory) = self.store.get_memory(memory_id)? {
                target_layer = Some(memory.memory_layer());
                if workflow_key.is_none() && memory.memory_layer() == MemoryLayer::Procedure {
                    workflow_key = memory.workflow_key_non_empty().map(ToString::to_string);
                }

                if !matches!(
                    memory.memory_layer(),
                    MemoryLayer::Trace | MemoryLayer::Procedure
                ) {
                    let mut updated =
                        memory.with_outcome_feedback(feedback.outcome, feedback.observed_at);
                    for (key, value) in &feedback.metadata {
                        updated
                            .metadata
                            .insert(format!("feedback_{key}"), value.clone());
                    }
                    self.store.put_memory(&updated)?;
                    stored_memory_ids.push(updated.memory_id.clone());
                }
            }
        }

        if let Some(belief_id) = target_belief_id.as_deref() {
            let updated_belief = self
                .store
                .get_belief_by_id(belief_id)?
                .map(|belief| belief.with_outcome_feedback(feedback.outcome, feedback.observed_at));
            if let Some(updated_belief) = updated_belief {
                target_layer = Some(MemoryLayer::Belief);
                self.store.update_belief(&updated_belief)?;
                if !stored_memory_ids
                    .iter()
                    .any(|existing| existing == &updated_belief.belief_id)
                {
                    stored_memory_ids.push(updated_belief.belief_id.clone());
                }
            }
        }

        if let Some(workflow_key_value) = workflow_key.as_deref() {
            let procedure_outcome = {
                let mut procedure_store = ProcedureStore::new(&mut self.store);
                procedure_store.record_feedback(
                    workflow_key_value,
                    feedback.outcome,
                    feedback.observed_at,
                )?
            };
            if let Some(procedure_outcome) = procedure_outcome {
                target_layer = Some(MemoryLayer::Procedure);
                if !stored_memory_ids
                    .iter()
                    .any(|existing| existing == &procedure_outcome.record.procedure_id)
                {
                    stored_memory_ids.push(procedure_outcome.record.procedure_id.clone());
                }
                if let Some(transition) = procedure_outcome.status_transition {
                    self.emit_procedure_status_transition(
                        None,
                        transition,
                        "feedback",
                        match feedback.outcome {
                            super::enums::OutcomeFeedbackKind::Positive => "positive_feedback",
                            super::enums::OutcomeFeedbackKind::Negative => "negative_feedback",
                        },
                    )?;
                }
            }
        }

        if stored_memory_ids.is_empty() {
            return Ok(None);
        }

        let mut details = BTreeMap::from([
            ("outcome".to_string(), feedback.outcome.as_str().to_string()),
            ("stored_memory_ids".to_string(), stored_memory_ids.join(",")),
        ]);
        if let Some(memory_id) = target_memory_id {
            details.insert("target_memory_id".to_string(), memory_id);
        }
        if let Some(belief_id) = target_belief_id {
            details.insert("target_belief_id".to_string(), belief_id);
        }
        if let Some(workflow_key_value) = workflow_key {
            details.insert("workflow_key".to_string(), workflow_key_value);
        }
        if let Some(layer) = target_layer {
            details.insert("target_layer".to_string(), layer.as_str().to_string());
        }
        for (key, value) in feedback.metadata {
            details.insert(format!("feedback_meta_{key}"), value);
        }

        self.audit.emit(AuditEvent {
            event_id: String::new(),
            occurred_at: self.clock.now(),
            action: "outcome_feedback_recorded".to_string(),
            candidate_id: None,
            memory_id: (target_layer != Some(MemoryLayer::Belief))
                .then(|| stored_memory_ids.first().cloned())
                .flatten(),
            belief_id: feedback.belief_id.clone(),
            query_text: None,
            details,
        });

        Ok(stored_memory_ids.into_iter().next())
    }

    #[must_use]
    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    fn touch_retrieved_memories(&mut self, hits: &[RetrievalHit]) -> Result<Vec<String>> {
        if !self.retrieval_touch_persistence_enabled() {
            return Ok(Vec::new());
        }
        let accessed_at = self.clock.now();
        let mut touched = Vec::new();
        let mut touches = Vec::new();
        let mut seen = std::collections::BTreeSet::new();

        for hit in hits {
            if hit.expired {
                continue;
            }
            let Some(memory_id) = hit.memory_id.as_deref() else {
                continue;
            };
            if !seen.insert(memory_id.to_string()) {
                continue;
            }
            let Some(memory) = self.store.get_memory(memory_id)? else {
                continue;
            };
            if memory.memory_layer() == MemoryLayer::Trace {
                continue;
            }

            touched.push(memory_id.to_string());
            touches.push((memory_id.to_string(), accessed_at));
        }

        if !touches.is_empty() {
            self.store.touch_memory_accesses(&touches)?;
        }

        Ok(touched)
    }

    fn retrieval_touch_persistence_enabled(&self) -> bool {
        self.promoter.policy().persist_retrieval_touches() && self.store.persists_access_touches()
    }

    fn persist_durable_memory(
        &mut self,
        candidate_id: Option<String>,
        mut memory: DurableMemory,
        supporting_episode_id: Option<&str>,
    ) -> Result<String> {
        let memory_layer = memory.memory_layer();
        if let Some(episode_id) = supporting_episode_id {
            memory = memory.with_supporting_episode(episode_id);
        }

        match memory_layer {
            MemoryLayer::Episode => {
                let episode_id = {
                    let mut episode_store = EpisodeStore::new(&mut self.store);
                    episode_store.save_memory(&memory)?
                };
                self.audit.emit(AuditEvent {
                    event_id: String::new(),
                    occurred_at: self.clock.now(),
                    action: "episode_stored".to_string(),
                    candidate_id,
                    memory_id: Some(episode_id.clone()),
                    belief_id: None,
                    query_text: None,
                    details: BTreeMap::from([(
                        "memory_layer".to_string(),
                        MemoryLayer::Episode.as_str().to_string(),
                    )]),
                });
                Ok(episode_id)
            }
            MemoryLayer::Belief => {
                let memory_id = self.store.put_memory(&memory)?;
                self.audit.emit(AuditEvent {
                    event_id: String::new(),
                    occurred_at: self.clock.now(),
                    action: "memory_stored".to_string(),
                    candidate_id: candidate_id.clone(),
                    memory_id: Some(memory_id.clone()),
                    belief_id: None,
                    query_text: None,
                    details: BTreeMap::from([
                        ("entity".to_string(), memory.entity.clone()),
                        ("slot".to_string(), memory.slot.clone()),
                        (
                            "memory_layer".to_string(),
                            memory.memory_layer().as_str().to_string(),
                        ),
                    ]),
                });

                let existing = {
                    let mut belief_store = BeliefStore::new(&mut self.store);
                    belief_store.get_current(&memory.entity, &memory.slot)?
                };
                let outcome =
                    self.belief_updater
                        .apply(existing.clone(), &memory, self.clock.as_ref());
                {
                    let mut belief_store = BeliefStore::new(&mut self.store);
                    if let Some(prior) = outcome.prior_belief.as_ref() {
                        belief_store.save(prior)?;
                    }
                    if let Some(current) = outcome.current_belief.as_ref() {
                        belief_store.save(current)?;
                    }
                }
                if outcome.current_belief.is_some() {
                    self.emit_belief_transition_audits(
                        candidate_id,
                        &memory,
                        &memory_id,
                        existing.as_ref(),
                        &outcome,
                    );
                }
                Ok(memory_id)
            }
            MemoryLayer::GoalState => {
                let goal_id = {
                    let mut goal_store = GoalStateStore::new(&mut self.store);
                    goal_store.save_memory(&memory, supporting_episode_id)?
                };
                self.audit.emit(AuditEvent {
                    event_id: String::new(),
                    occurred_at: self.clock.now(),
                    action: "goal_state_stored".to_string(),
                    candidate_id,
                    memory_id: Some(goal_id.clone()),
                    belief_id: None,
                    query_text: None,
                    details: BTreeMap::from([(
                        "memory_layer".to_string(),
                        MemoryLayer::GoalState.as_str().to_string(),
                    )]),
                });
                Ok(goal_id)
            }
            MemoryLayer::SelfModel => {
                let self_model_id = {
                    let mut self_model_store = SelfModelStore::new(&mut self.store);
                    self_model_store.save_memory(&memory, supporting_episode_id)?
                };
                self.audit.emit(AuditEvent {
                    event_id: String::new(),
                    occurred_at: self.clock.now(),
                    action: "self_model_stored".to_string(),
                    candidate_id,
                    memory_id: Some(self_model_id.clone()),
                    belief_id: None,
                    query_text: None,
                    details: BTreeMap::from([(
                        "memory_layer".to_string(),
                        MemoryLayer::SelfModel.as_str().to_string(),
                    )]),
                });
                Ok(self_model_id)
            }
            MemoryLayer::Procedure => {
                let procedure_id = {
                    let mut procedure_store = ProcedureStore::new(&mut self.store);
                    procedure_store.save_memory(&memory)?
                };
                self.audit.emit(AuditEvent {
                    event_id: String::new(),
                    occurred_at: self.clock.now(),
                    action: "procedure_stored".to_string(),
                    candidate_id,
                    memory_id: Some(procedure_id.clone()),
                    belief_id: None,
                    query_text: None,
                    details: BTreeMap::from([(
                        "memory_layer".to_string(),
                        MemoryLayer::Procedure.as_str().to_string(),
                    )]),
                });
                Ok(procedure_id)
            }
            MemoryLayer::Trace => {
                let mut trace_meta = BTreeMap::from([(
                    "memory_layer".to_string(),
                    MemoryLayer::Trace.as_str().to_string(),
                )]);
                if !memory.entity.is_empty() {
                    trace_meta.insert("entity".to_string(), memory.entity.clone());
                }
                if !memory.slot.is_empty() {
                    trace_meta.insert("slot".to_string(), memory.slot.clone());
                }
                self.store.put_trace(&memory.raw_text, trace_meta)
            }
        }
    }

    fn emit_policy_rejection(
        &self,
        candidate: &CandidateMemory,
        promotion: &super::schemas::PromotionResult,
        reason_code: ReasonCode,
    ) {
        let mut details = BTreeMap::new();
        for key in [
            "route_basis",
            "fallback_layer",
            "policy_version",
            "score_threshold",
        ] {
            if let Some(value) = promotion.details.get(key) {
                details.insert(key.to_string(), value.clone());
            }
        }
        self.emit_policy_rejection_event(
            Some(candidate.candidate_id.clone()),
            candidate.memory_layer(),
            promotion.decision,
            promotion.reason.clone(),
            reason_code,
            details,
        );
    }

    fn emit_policy_rejection_event(
        &self,
        candidate_id: Option<String>,
        target_layer: MemoryLayer,
        decision: PromotionDecision,
        reason: String,
        reason_code: ReasonCode,
        mut details: BTreeMap<String, String>,
    ) {
        details.insert(
            "target_layer".to_string(),
            target_layer.as_str().to_string(),
        );
        details.insert(
            "decision".to_string(),
            format!("{decision:?}").to_lowercase(),
        );
        details.insert("reason".to_string(), reason);
        details.insert("reason_code".to_string(), reason_code.as_str().to_string());
        self.audit.emit(AuditEvent {
            event_id: String::new(),
            occurred_at: self.clock.now(),
            action: "policy_rejected".to_string(),
            candidate_id,
            memory_id: None,
            belief_id: None,
            query_text: None,
            details,
        });
    }

    fn emit_belief_transition_audits(
        &self,
        candidate_id: Option<String>,
        memory: &DurableMemory,
        memory_id: &str,
        existing: Option<&BeliefRecord>,
        outcome: &BeliefUpdateOutcome,
    ) {
        let Some(current) = outcome.current_belief.as_ref() else {
            return;
        };

        let mut details = BTreeMap::from([
            ("entity".to_string(), current.entity.clone()),
            ("slot".to_string(), current.slot.clone()),
            ("value".to_string(), current.current_value.clone()),
            (
                "action".to_string(),
                format!("{:?}", outcome.action).to_lowercase(),
            ),
            ("status".to_string(), current.status.as_str().to_string()),
            (
                "belief_retrieval_status".to_string(),
                current.view_status().as_str().to_string(),
            ),
            (
                "supporting_memory_count".to_string(),
                current.supporting_memory_ids.len().to_string(),
            ),
            (
                "opposing_memory_count".to_string(),
                current.opposing_memory_ids.len().to_string(),
            ),
            (
                "contradictions_observed".to_string(),
                current.contradictions_observed.to_string(),
            ),
        ]);
        if let Some(last_contradiction_at) = current.last_contradiction_at {
            details.insert(
                "last_contradiction_at".to_string(),
                last_contradiction_at.to_rfc3339(),
            );
        }
        if let Some(seconds) = current.time_to_last_resolution_seconds {
            details.insert(
                "time_to_last_resolution_seconds".to_string(),
                seconds.to_string(),
            );
        }

        self.audit.emit(AuditEvent {
            event_id: String::new(),
            occurred_at: self.clock.now(),
            action: "belief_updated".to_string(),
            candidate_id: candidate_id.clone(),
            memory_id: Some(memory_id.to_string()),
            belief_id: Some(current.belief_id.clone()),
            query_text: None,
            details,
        });

        match outcome.action {
            BeliefAction::Reinforce => {
                let mut details = BTreeMap::from([
                    ("entity".to_string(), current.entity.clone()),
                    ("slot".to_string(), current.slot.clone()),
                    ("value".to_string(), current.current_value.clone()),
                    (
                        "resolved_contestation".to_string(),
                        (existing.is_some_and(|belief| {
                            belief.status == super::enums::BeliefStatus::Disputed
                        }) && current.status == super::enums::BeliefStatus::Active)
                            .to_string(),
                    ),
                ]);
                if let Some(seconds) = current.time_to_last_resolution_seconds {
                    details.insert(
                        "time_to_last_resolution_seconds".to_string(),
                        seconds.to_string(),
                    );
                }
                self.audit.emit(AuditEvent {
                    event_id: String::new(),
                    occurred_at: self.clock.now(),
                    action: "belief_reinforced".to_string(),
                    candidate_id: candidate_id.clone(),
                    memory_id: Some(memory_id.to_string()),
                    belief_id: Some(current.belief_id.clone()),
                    query_text: None,
                    details,
                });
            }
            BeliefAction::Dispute => {
                let existing_trust = existing
                    .map(BeliefRecord::strongest_source_weight)
                    .unwrap_or(0.0);
                let mut details = BTreeMap::from([
                    ("entity".to_string(), current.entity.clone()),
                    ("slot".to_string(), current.slot.clone()),
                    (
                        "prior_value".to_string(),
                        existing
                            .map(|belief| belief.current_value.clone())
                            .unwrap_or_default(),
                    ),
                    ("new_value".to_string(), memory.value.clone()),
                    (
                        "trust_comparison".to_string(),
                        format!(
                            "new={:.2},existing={:.2}",
                            memory.source.trust_weight, existing_trust
                        ),
                    ),
                    (
                        "contradictions_observed".to_string(),
                        current.contradictions_observed.to_string(),
                    ),
                ]);
                if let Some(last_contradiction_at) = current.last_contradiction_at {
                    details.insert(
                        "last_contradiction_at".to_string(),
                        last_contradiction_at.to_rfc3339(),
                    );
                }
                self.audit.emit(AuditEvent {
                    event_id: String::new(),
                    occurred_at: self.clock.now(),
                    action: "belief_contradiction_detected".to_string(),
                    candidate_id: candidate_id.clone(),
                    memory_id: Some(memory_id.to_string()),
                    belief_id: Some(current.belief_id.clone()),
                    query_text: None,
                    details,
                });
            }
            BeliefAction::Update if outcome.prior_belief.is_some() => {
                let Some(prior) = outcome.prior_belief.as_ref() else {
                    return;
                };
                let mut details = BTreeMap::from([
                    ("entity".to_string(), current.entity.clone()),
                    ("slot".to_string(), current.slot.clone()),
                    ("prior_value".to_string(), prior.current_value.clone()),
                    ("new_value".to_string(), current.current_value.clone()),
                    (
                        "previous_status".to_string(),
                        prior.status.as_str().to_string(),
                    ),
                    (
                        "trust_comparison".to_string(),
                        format!(
                            "new={:.2},existing={:.2}",
                            memory.source.trust_weight,
                            existing
                                .map(BeliefRecord::strongest_source_weight)
                                .unwrap_or(0.0)
                        ),
                    ),
                ]);
                if let Some(seconds) = current.time_to_last_resolution_seconds {
                    details.insert(
                        "time_to_last_resolution_seconds".to_string(),
                        seconds.to_string(),
                    );
                }
                self.audit.emit(AuditEvent {
                    event_id: String::new(),
                    occurred_at: self.clock.now(),
                    action: "belief_replaced".to_string(),
                    candidate_id: candidate_id.clone(),
                    memory_id: Some(memory_id.to_string()),
                    belief_id: Some(current.belief_id.clone()),
                    query_text: None,
                    details,
                });
            }
            BeliefAction::Retract => {
                self.audit.emit(AuditEvent {
                    event_id: String::new(),
                    occurred_at: self.clock.now(),
                    action: "belief_retracted".to_string(),
                    candidate_id: candidate_id.clone(),
                    memory_id: Some(memory_id.to_string()),
                    belief_id: Some(current.belief_id.clone()),
                    query_text: None,
                    details: BTreeMap::from([
                        ("entity".to_string(), current.entity.clone()),
                        ("slot".to_string(), current.slot.clone()),
                        ("value".to_string(), current.current_value.clone()),
                    ]),
                });
            }
            BeliefAction::Update => {}
        }
    }

    fn govern_self_model_memory(
        &mut self,
        mut memory: DurableMemory,
    ) -> Result<SelfModelGovernanceDecision> {
        let Some(entity) = memory.entity_non_empty().map(ToString::to_string) else {
            return Ok(SelfModelGovernanceDecision::Persist(memory));
        };
        let Some(slot) = memory.slot_non_empty().map(ToString::to_string) else {
            return Ok(SelfModelGovernanceDecision::Persist(memory));
        };
        let Some(value) = memory.value_non_empty().map(ToString::to_string) else {
            return Ok(SelfModelGovernanceDecision::Persist(memory));
        };

        let kind = SelfModelKind::from_slot(&slot);
        let stability_class = kind.stability_class();
        let update_requirement = kind.update_requirement();
        memory.internal_layer = Some(MemoryLayer::SelfModel);
        memory.entity = entity.clone();
        memory.slot = slot.clone();
        memory.value = value.clone();
        memory
            .metadata
            .insert("self_model_kind".to_string(), kind.as_str().to_string());
        memory.metadata.insert(
            "self_model_stability_class".to_string(),
            stability_class.as_str().to_string(),
        );
        memory.metadata.insert(
            "self_model_update_requirement".to_string(),
            update_requirement.as_str().to_string(),
        );

        let existing = {
            let mut self_model_store = SelfModelStore::new(&mut self.store);
            self_model_store.get_latest_for_entity_slot(&entity, &slot)?
        };
        let corroborating_count = {
            let mut self_model_store = SelfModelStore::new(&mut self.store);
            self_model_store
                .matching_values(&entity, &slot, &value)?
                .len()
        };

        if let Some(existing_record) = existing {
            if existing_record.stability_class == SelfModelStabilityClass::StableDirective
                && existing_record.value != value
            {
                let trusted_update = self
                    .promoter
                    .policy_profile()
                    .allows_singleton_self_model_from_trusted_source(
                        memory.source.source_type,
                        memory.source.trust_weight,
                    );
                let corroborated_update = corroborating_count
                    >= self
                        .promoter
                        .policy_profile()
                        .minimum_stable_directive_update_evidence();
                if self
                    .promoter
                    .policy_profile()
                    .stable_directive_requires_trusted_update_path()
                    && !(trusted_update || corroborated_update)
                {
                    return Ok(SelfModelGovernanceDecision::Downgrade {
                        reason: "stable directives require a trusted update path or corroborated evidence"
                            .to_string(),
                        reason_code: ReasonCode::StableDirectiveUpdateRejected,
                    });
                }
                memory.metadata.insert(
                    "stable_directive_update_path".to_string(),
                    if trusted_update {
                        "trusted_source".to_string()
                    } else {
                        "corroborated_evidence".to_string()
                    },
                );
            }
        } else if stability_class == SelfModelStabilityClass::StableDirective {
            let trusted_seed = self
                .promoter
                .policy_profile()
                .allows_singleton_self_model_from_trusted_source(
                    memory.source.source_type,
                    memory.source.trust_weight,
                );
            let corroborated_seed = corroborating_count + 1
                >= self
                    .promoter
                    .policy_profile()
                    .minimum_stable_directive_update_evidence();
            if !(trusted_seed || corroborated_seed) {
                return Ok(SelfModelGovernanceDecision::Downgrade {
                    reason: "stable directives require a trusted source or corroborated evidence"
                        .to_string(),
                    reason_code: ReasonCode::StableDirectiveUpdateRejected,
                });
            }
        }

        Ok(SelfModelGovernanceDecision::Persist(memory))
    }

    fn build_promotion_context(&mut self, candidate: &CandidateMemory) -> Result<PromotionContext> {
        let mut context = PromotionContext {
            verified_source: Self::is_verified_source(candidate),
            seeded_by_system: Self::is_seeded_by_system(candidate),
            goal_state_evidence_count: usize::from(
                candidate.memory_layer() == MemoryLayer::GoalState,
            ),
            ..PromotionContext::default()
        };

        if let (Some(entity), Some(slot), Some(value)) = (
            candidate.entity_non_empty(),
            candidate.slot_non_empty(),
            candidate.value_non_empty(),
        ) {
            let belief_memories = self.store.list_memories_for_belief(entity, slot)?;
            let corroborating_beliefs = belief_memories
                .iter()
                .filter(|memory| !memory.is_retraction)
                .filter(|memory| memory.value == value)
                .count();
            let contradictory_beliefs = belief_memories
                .iter()
                .filter(|memory| !memory.is_retraction)
                .filter(|memory| memory.value != value)
                .count();
            context.belief_evidence_count = corroborating_beliefs
                + usize::from(candidate.memory_layer() == MemoryLayer::Belief);

            context.self_model_evidence_count =
                {
                    let mut self_model_store = SelfModelStore::new(&mut self.store);
                    let corroborating_self_model =
                        self_model_store.matching_values(entity, slot, value)?.len();
                    let contradictory_self_model = self_model_store
                        .list_for_entity_memories(entity)?
                        .into_iter()
                        .filter(|memory| memory.slot == slot)
                        .filter(|memory| memory.value != value)
                        .count();
                    if candidate.memory_layer() == MemoryLayer::SelfModel {
                        context.corroborating_evidence_count = corroborating_self_model;
                        context.contradictory_evidence_count = contradictory_self_model;
                    }
                    corroborating_self_model
                } + usize::from(candidate.memory_layer() == MemoryLayer::SelfModel);

            if candidate.memory_layer() == MemoryLayer::Belief {
                context.corroborating_evidence_count = corroborating_beliefs;
                context.contradictory_evidence_count = contradictory_beliefs;
            }

            context.goal_state_evidence_count =
                if candidate.memory_layer() == MemoryLayer::GoalState {
                    self.store
                        .list_memory_versions_by_layer(MemoryLayer::GoalState)?
                        .into_iter()
                        .filter(|memory| memory.has_required_structure_for(MemoryLayer::GoalState))
                        .filter(|memory| memory.entity == entity)
                        .filter(|memory| memory.slot == slot)
                        .filter(|memory| memory.value == value)
                        .count()
                        + 1
                } else {
                    context.goal_state_evidence_count
                };
        }

        if let Some(workflow_key) = candidate.workflow_key_non_empty() {
            let workflow_episodes = {
                let mut episode_store = EpisodeStore::new(&mut self.store);
                episode_store.list_by_workflow_key(workflow_key)?
            };
            context.procedure_success_count = workflow_episodes
                .iter()
                .filter(|record| Self::is_success_outcome(record.outcome.as_deref()))
                .count()
                + usize::from(Self::is_success_outcome(
                    candidate.metadata.get("outcome").map(String::as_str),
                ));
            context.procedure_failure_count = workflow_episodes
                .iter()
                .filter(|record| Self::is_failure_outcome(record.outcome.as_deref()))
                .count()
                + usize::from(Self::is_failure_outcome(
                    candidate.metadata.get("outcome").map(String::as_str),
                ));
        }

        Ok(context)
    }

    fn emit_procedure_status_transition(
        &mut self,
        candidate_id: Option<String>,
        transition: ProcedureStatusTransition,
        source: &str,
        reason: &str,
    ) -> Result<String> {
        let trace_id = self.store.put_trace(
            &format!(
                "Procedure {workflow_key} transitioned from {previous} to {next} because of {reason}",
                workflow_key = transition.workflow_key,
                previous = transition.previous_status.as_str(),
                next = transition.next_status.as_str(),
                reason = reason,
            ),
            BTreeMap::from([
                ("action".to_string(), "procedure_status_changed".to_string()),
                (
                    "memory_layer".to_string(),
                    MemoryLayer::Procedure.as_str().to_string(),
                ),
                ("entity".to_string(), "procedure".to_string()),
                ("slot".to_string(), transition.workflow_key.clone()),
                (
                    "value".to_string(),
                    transition.next_status.as_str().to_string(),
                ),
                ("workflow_key".to_string(), transition.workflow_key.clone()),
                ("procedure_id".to_string(), transition.procedure_id.clone()),
                (
                    "previous_status".to_string(),
                    transition.previous_status.as_str().to_string(),
                ),
                (
                    "next_status".to_string(),
                    transition.next_status.as_str().to_string(),
                ),
                ("source".to_string(), source.to_string()),
                ("transition_reason".to_string(), reason.to_string()),
                ("occurred_at".to_string(), self.clock.now().to_rfc3339()),
                ("source_type".to_string(), "system".to_string()),
            ]),
        )?;
        self.audit.emit(AuditEvent {
            event_id: String::new(),
            occurred_at: self.clock.now(),
            action: "procedure_status_changed".to_string(),
            candidate_id,
            memory_id: Some(transition.procedure_id),
            belief_id: None,
            query_text: None,
            details: BTreeMap::from([
                ("workflow_key".to_string(), transition.workflow_key),
                (
                    "previous_status".to_string(),
                    transition.previous_status.as_str().to_string(),
                ),
                (
                    "next_status".to_string(),
                    transition.next_status.as_str().to_string(),
                ),
                ("transition_trace_id".to_string(), trace_id.clone()),
                ("source".to_string(), source.to_string()),
                ("transition_reason".to_string(), reason.to_string()),
            ]),
        });
        Ok(trace_id)
    }

    fn transition_reason_from_consolidation(
        record: &super::schemas::ConsolidationRecord,
    ) -> &'static str {
        match record.metadata.get("outcome").map(String::as_str) {
            Some("failure") => "failure",
            Some("success") => "success",
            _ => "consolidation",
        }
    }

    fn reconcile_procedure_statuses(&mut self, candidate_id: Option<String>) -> Result<()> {
        let lifecycle_transitions = {
            let mut procedure_store = ProcedureStore::new(&mut self.store);
            procedure_store.sync_all_effective_statuses(self.clock.now())?
        };
        for transition in lifecycle_transitions {
            self.emit_procedure_status_transition(
                candidate_id.clone(),
                transition,
                "reconciliation",
                "reconciliation",
            )?;
        }
        Ok(())
    }

    fn is_verified_source(candidate: &CandidateMemory) -> bool {
        candidate
            .metadata
            .get("verified_source")
            .or_else(|| candidate.metadata.get("verified"))
            .is_some_and(|value| value == "true")
    }

    fn is_seeded_by_system(candidate: &CandidateMemory) -> bool {
        candidate.source.source_type == SourceType::System
            && (candidate.memory_layer() == MemoryLayer::Procedure
                || candidate
                    .metadata
                    .get("seeded_by_system")
                    .is_some_and(|value| value == "true")
                || candidate
                    .metadata
                    .get("seeded")
                    .is_some_and(|value| value == "true"))
    }

    fn is_success_outcome(value: Option<&str>) -> bool {
        value.is_some_and(|text| {
            let lower = text.to_lowercase();
            lower.contains("success")
                || lower.contains("completed")
                || lower.contains("passed")
                || lower.contains("ok")
        })
    }

    fn is_failure_outcome(value: Option<&str>) -> bool {
        value.is_some_and(|text| {
            let lower = text.to_lowercase();
            lower.contains("fail")
                || lower.contains("error")
                || lower.contains("blocked")
                || lower.contains("aborted")
                || lower.contains("timeout")
        })
    }
}
