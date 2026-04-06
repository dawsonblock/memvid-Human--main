use std::collections::BTreeMap;
use std::sync::Arc;

use super::adapters::memvid_store::MemoryStore;
use super::audit::AuditLogger;
use super::belief_store::BeliefStore;
use super::belief_updater::BeliefUpdater;
use super::clock::Clock;
use super::enums::{MemoryType, PromotionDecision};
use super::errors::Result;
use super::memory_classifier::MemoryClassifier;
use super::memory_promoter::MemoryPromoter;
use super::memory_retriever::MemoryRetriever;
use super::schemas::{AuditEvent, CandidateMemory, RetrievalHit, RetrievalQuery};

/// Single governed write/read authority for agent memory.
pub struct MemoryController<S: MemoryStore> {
    store: S,
    clock: Arc<dyn Clock>,
    audit: AuditLogger,
    classifier: MemoryClassifier,
    promoter: MemoryPromoter,
    belief_updater: BeliefUpdater,
    retriever: MemoryRetriever,
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
                let Some(memory) = promotion.durable_memory else {
                    return Ok(None);
                };
                let memory_id = self.store.put_memory(&memory)?;
                self.audit.emit(AuditEvent {
                    event_id: String::new(),
                    occurred_at: self.clock.now(),
                    action: "memory_stored".to_string(),
                    candidate_id: Some(classified.candidate_id),
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

                if memory.memory_type != MemoryType::Episode
                    && memory.memory_type != MemoryType::Trace
                {
                    let mut belief_store = BeliefStore::new(&mut self.store);
                    let existing = belief_store.get(&memory.entity, &memory.slot)?;
                    let outcome = self
                        .belief_updater
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
}
