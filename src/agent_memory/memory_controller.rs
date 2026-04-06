use std::collections::BTreeMap;
use std::sync::Arc;

use super::adapters::memvid_store::MemoryStore;
use super::audit::AuditLogger;
use super::belief_store::BeliefStore;
use super::belief_updater::BeliefUpdater;
use super::clock::Clock;
use super::consolidation_engine::ConsolidationEngine;
use super::enums::{MemoryLayer, MemoryType, PromotionDecision};
use super::episode_store::EpisodeStore;
use super::errors::Result;
use super::goal_state_store::GoalStateStore;
use super::memory_classifier::MemoryClassifier;
use super::memory_promoter::MemoryPromoter;
use super::memory_retriever::MemoryRetriever;
use super::procedure_store::{ProcedureStatusTransition, ProcedureStore};
use super::schemas::{AuditEvent, CandidateMemory, RetrievalHit, RetrievalQuery};
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
        Self {
            store,
            clock,
            audit,
            classifier,
            promoter,
            belief_updater,
            retriever,
            consolidation_engine: ConsolidationEngine,
        }
    }

    pub fn ingest(&mut self, candidate: CandidateMemory) -> Result<Option<String>> {
        let classified = self.classifier.classify(candidate);
        self.audit.emit(AuditEvent {
            event_id: String::new(),
            occurred_at: self.clock.now(),
            action: "classification".to_string(),
            candidate_id: Some(classified.candidate_id.clone()),
            memory_id: None,
            belief_id: None,
            query_text: None,
            details: BTreeMap::from([
                (
                    "memory_type".to_string(),
                    format!("{:?}", classified.memory_type).to_lowercase(),
                ),
                (
                    "memory_layer".to_string(),
                    classified.memory_layer().as_str().to_string(),
                ),
                ("entity".to_string(), classified.entity.clone()),
                ("slot".to_string(), classified.slot.clone()),
            ]),
        });

        let promotion = self.promoter.promote(&classified, self.clock.as_ref());
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
            details: BTreeMap::from([
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
            ]),
        });

        match promotion.decision {
            PromotionDecision::Reject => Ok(None),
            PromotionDecision::StoreTrace => {
                let trace_id = self.store.put_trace(
                    &classified.raw_text,
                    BTreeMap::from([
                        ("entity".to_string(), classified.entity.clone()),
                        ("slot".to_string(), classified.slot.clone()),
                        (
                            "memory_type".to_string(),
                            format!("{:?}", MemoryType::Trace).to_lowercase(),
                        ),
                        ("memory_layer".to_string(), "trace".to_string()),
                    ]),
                )?;
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

                let memory_id = match memory_layer {
                    MemoryLayer::Episode => {
                        let episode_id = {
                            let mut episode_store = EpisodeStore::new(&mut self.store);
                            episode_store.save_memory(&memory)?
                        };
                        self.audit.emit(AuditEvent {
                            event_id: String::new(),
                            occurred_at: self.clock.now(),
                            action: "episode_stored".to_string(),
                            candidate_id: Some(classified.candidate_id.clone()),
                            memory_id: Some(episode_id.clone()),
                            belief_id: None,
                            query_text: None,
                            details: BTreeMap::from([(
                                "memory_layer".to_string(),
                                MemoryLayer::Episode.as_str().to_string(),
                            )]),
                        });
                        episode_id
                    }
                    MemoryLayer::Belief => {
                        let memory_id = self.store.put_memory(&memory)?;
                        self.audit.emit(AuditEvent {
                            event_id: String::new(),
                            occurred_at: self.clock.now(),
                            action: "memory_stored".to_string(),
                            candidate_id: Some(classified.candidate_id.clone()),
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

                        let mut belief_store = BeliefStore::new(&mut self.store);
                        let existing = belief_store.get(&memory.entity, &memory.slot)?;
                        let outcome =
                            self.belief_updater
                                .apply(existing, &memory, self.clock.as_ref());
                        if let Some(prior) = outcome.prior_belief.as_ref() {
                            belief_store.save(prior)?;
                        }
                        if let Some(current) = outcome.current_belief.as_ref() {
                            belief_store.save(current)?;
                            self.audit.emit(AuditEvent {
                                event_id: String::new(),
                                occurred_at: self.clock.now(),
                                action: "belief_updated".to_string(),
                                candidate_id: None,
                                memory_id: Some(memory_id.clone()),
                                belief_id: Some(current.belief_id.clone()),
                                query_text: None,
                                details: BTreeMap::from([
                                    (
                                        "action".to_string(),
                                        format!("{:?}", outcome.action).to_lowercase(),
                                    ),
                                    (
                                        "status".to_string(),
                                        format!("{:?}", current.status).to_lowercase(),
                                    ),
                                ]),
                            });
                        }
                        memory_id
                    }
                    MemoryLayer::GoalState => {
                        let goal_id = {
                            let mut goal_store = GoalStateStore::new(&mut self.store);
                            goal_store.save_memory(
                                &memory,
                                episode_memory
                                    .as_ref()
                                    .map(|episode| episode.memory_id.as_str()),
                            )?
                        };
                        self.audit.emit(AuditEvent {
                            event_id: String::new(),
                            occurred_at: self.clock.now(),
                            action: "goal_state_stored".to_string(),
                            candidate_id: Some(classified.candidate_id.clone()),
                            memory_id: Some(goal_id.clone()),
                            belief_id: None,
                            query_text: None,
                            details: BTreeMap::from([(
                                "memory_layer".to_string(),
                                MemoryLayer::GoalState.as_str().to_string(),
                            )]),
                        });
                        goal_id
                    }
                    MemoryLayer::SelfModel => {
                        let self_model_id = {
                            let mut self_model_store = SelfModelStore::new(&mut self.store);
                            self_model_store.save_memory(
                                &memory,
                                episode_memory
                                    .as_ref()
                                    .map(|episode| episode.memory_id.as_str()),
                            )?
                        };
                        self.audit.emit(AuditEvent {
                            event_id: String::new(),
                            occurred_at: self.clock.now(),
                            action: "self_model_stored".to_string(),
                            candidate_id: Some(classified.candidate_id.clone()),
                            memory_id: Some(self_model_id.clone()),
                            belief_id: None,
                            query_text: None,
                            details: BTreeMap::from([(
                                "memory_layer".to_string(),
                                MemoryLayer::SelfModel.as_str().to_string(),
                            )]),
                        });
                        self_model_id
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
                            candidate_id: Some(classified.candidate_id.clone()),
                            memory_id: Some(procedure_id.clone()),
                            belief_id: None,
                            query_text: None,
                            details: BTreeMap::from([(
                                "memory_layer".to_string(),
                                MemoryLayer::Procedure.as_str().to_string(),
                            )]),
                        });
                        procedure_id
                    }
                    MemoryLayer::Trace => self.store.put_trace(
                        &memory.raw_text,
                        BTreeMap::from([
                            ("entity".to_string(), memory.entity.clone()),
                            ("slot".to_string(), memory.slot.clone()),
                            (
                                "memory_layer".to_string(),
                                MemoryLayer::Trace.as_str().to_string(),
                            ),
                        ]),
                    )?,
                };

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

                let lifecycle_transitions = {
                    let mut procedure_store = ProcedureStore::new(&mut self.store);
                    procedure_store.sync_all_effective_statuses(self.clock.now())?
                };
                for transition in lifecycle_transitions {
                    self.emit_procedure_status_transition(
                        Some(classified.candidate_id.clone()),
                        transition,
                        "reconciliation",
                        "reconciliation",
                    )?;
                }

                Ok(Some(memory_id))
            }
        }
    }

    pub fn retrieve(&mut self, query: RetrievalQuery) -> Result<Vec<RetrievalHit>> {
        let hits = self
            .retriever
            .retrieve(&mut self.store, &query, self.clock.as_ref())?;
        self.audit.emit(AuditEvent {
            event_id: String::new(),
            occurred_at: self.clock.now(),
            action: "retrieval".to_string(),
            candidate_id: None,
            memory_id: None,
            belief_id: None,
            query_text: Some(query.query_text.clone()),
            details: BTreeMap::from([
                (
                    "intent".to_string(),
                    format!("{:?}", query.intent).to_lowercase(),
                ),
                ("hits".to_string(), hits.len().to_string()),
            ]),
        });
        Ok(hits)
    }

    #[must_use]
    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
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
}
