use super::adapters::memvid_store::MemoryStore;
use super::clock::Clock;
use super::enums::{MemoryLayer, QueryIntent};
use super::errors::Result;
use super::ranker::Ranker;
use super::retention::RetentionManager;
use super::schemas::{RetrievalHit, RetrievalQuery};

/// Read orchestrator for governed retrieval.
#[derive(Debug, Clone)]
pub struct MemoryRetriever {
    ranker: Ranker,
    retention: RetentionManager,
}

impl MemoryRetriever {
    #[must_use]
    pub fn new(ranker: Ranker, retention: RetentionManager) -> Self {
        Self { ranker, retention }
    }

    pub fn retrieve<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        clock: &dyn Clock,
    ) -> Result<Vec<RetrievalHit>> {
        let now = clock.now();
        let mut hits = Vec::new();

        match query.intent {
            QueryIntent::CurrentFact => {
                if let (Some(entity), Some(slot)) = (query.entity.as_deref(), query.slot.as_deref())
                {
                    if let Some(belief) = store.get_active_belief(entity, slot)? {
                        hits.push(RetrievalHit {
                            memory_id: belief.supporting_memory_ids.last().cloned(),
                            belief_id: Some(belief.belief_id.clone()),
                            entity: Some(belief.entity.clone()),
                            slot: Some(belief.slot.clone()),
                            value: Some(belief.current_value.clone()),
                            text: belief.current_value,
                            memory_layer: Some(MemoryLayer::Belief),
                            memory_type: None,
                            score: belief.confidence,
                            timestamp: belief.last_reviewed_at,
                            scope: query.scope,
                            source: None,
                            from_belief: true,
                            expired: false,
                            metadata: Default::default(),
                        });
                    }
                }
                hits.extend(store.search(query)?);
            }
            QueryIntent::HistoricalFact => {
                if let (Some(entity), Some(slot), Some(as_of)) =
                    (query.entity.as_deref(), query.slot.as_deref(), query.as_of)
                {
                    let memories = store.list_memories_for_belief(entity, slot)?;
                    let mut historical: Vec<_> = memories
                        .into_iter()
                        .filter(|memory| {
                            let ts = memory
                                .event_at
                                .or(memory.valid_from)
                                .unwrap_or(memory.stored_at);
                            ts <= as_of
                        })
                        .collect();
                    historical.sort_by(|left, right| {
                        right
                            .event_at
                            .or(right.valid_from)
                            .unwrap_or(right.stored_at)
                            .cmp(&left.event_at.or(left.valid_from).unwrap_or(left.stored_at))
                    });
                    if let Some(memory) = historical.into_iter().next() {
                        let expired = self.retention.evaluate(&memory, now).expired;
                        hits.push(RetrievalHit {
                            memory_id: Some(memory.memory_id.clone()),
                            belief_id: None,
                            entity: Some(memory.entity.clone()),
                            slot: Some(memory.slot.clone()),
                            value: Some(memory.value.clone()),
                            text: memory.raw_text.clone(),
                            memory_layer: Some(memory.memory_layer()),
                            memory_type: Some(memory.memory_type),
                            score: memory.confidence + 0.8,
                            timestamp: memory
                                .event_at
                                .or(memory.valid_from)
                                .unwrap_or(memory.stored_at),
                            scope: Some(memory.scope),
                            source: Some(memory.source.source_type),
                            from_belief: false,
                            expired,
                            metadata: memory.metadata.clone(),
                        });
                    }
                }
                hits.extend(store.search(query)?);
            }
            QueryIntent::PreferenceLookup
            | QueryIntent::TaskState
            | QueryIntent::EpisodicRecall
            | QueryIntent::SemanticBackground => {
                hits.extend(store.search(query)?);
            }
        }

        let filtered = if query.include_expired {
            hits
        } else {
            hits.into_iter().filter(|hit| !hit.expired).collect()
        };

        let mut ranked = self.ranker.rerank(filtered, query.intent, now);
        ranked.truncate(query.top_k);
        Ok(ranked)
    }
}
