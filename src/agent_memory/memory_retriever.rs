use std::collections::HashSet;

use super::adapters::memvid_store::MemoryStore;
use super::clock::Clock;
use super::enums::{MemoryLayer, ProcedureStatus, QueryIntent};
use super::episode_store::EpisodeStore;
use super::errors::Result;
use super::goal_state_store::GoalStateStore;
use super::procedure_store::ProcedureStore;
use super::ranker::Ranker;
use super::retention::RetentionManager;
use super::schemas::{DurableMemory, ProcedureRecord, RetrievalHit, RetrievalQuery};
use super::self_model_store::SelfModelStore;

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
                let direct_hits = match query.intent {
                    QueryIntent::PreferenceLookup => {
                        self.preference_hits(store, query, now)?
                    }
                    QueryIntent::TaskState => self.task_state_hits(store, query, now)?,
                    QueryIntent::EpisodicRecall => self.episodic_hits(store, query, now)?,
                    QueryIntent::SemanticBackground
                    | QueryIntent::CurrentFact
                    | QueryIntent::HistoricalFact => Vec::new(),
                };
                if direct_hits.is_empty() {
                    hits.extend(store.search(query)?);
                } else {
                    hits.extend(direct_hits);
                }
            }
        }

        let mut filtered = self.filter_and_dedup_hits(hits, query);
        if !query.include_expired {
            filtered.retain(|hit| !hit.expired);
        }
        if query.intent == QueryIntent::TaskState {
            filtered.retain(|hit| !Self::is_retired_procedure_hit(hit));
        }

        let mut ranked = self.ranker.rerank(filtered, query.intent, now);
        ranked.truncate(query.top_k);
        Ok(ranked)
    }

    fn preference_hits<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<RetrievalHit>> {
        let Some(entity) = query.entity.as_deref() else {
            return Ok(Vec::new());
        };

        let memories: Vec<DurableMemory> = {
            let mut self_model_store = SelfModelStore::new(store);
            let memories = self_model_store.list_for_entity_memories(entity)?;
            if let Some(slot) = query.slot.as_deref() {
                memories
                    .into_iter()
                    .find(|memory| memory.slot == slot)
                    .into_iter()
                    .collect()
            } else {
                let mut seen_slots = HashSet::new();
                memories
                    .into_iter()
                    .filter(|memory| seen_slots.insert(memory.slot.clone()))
                    .collect()
            }
        };

        Ok(memories
            .into_iter()
            .map(|memory| self.hit_from_memory(&memory, query, now))
            .collect())
    }

    fn task_state_hits<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<RetrievalHit>> {
        let mut hits = Vec::new();

        let goal_memories: Vec<_> = {
            let mut goal_store = GoalStateStore::new(store);
            goal_store
                .list_active_memories()?
                .into_iter()
                .filter(|memory| {
                    query
                        .entity
                        .as_deref()
                        .is_none_or(|entity| memory.entity == entity)
                })
                .filter(|memory| {
                    query
                        .slot
                        .as_deref()
                        .is_none_or(|slot| memory.slot == slot)
                })
                .collect()
        };
        for memory in &goal_memories {
            hits.push(self.hit_from_memory(memory, query, now));
        }

        let supporting_episode_ids: Vec<_> = goal_memories
            .iter()
            .flat_map(Self::supporting_episode_ids)
            .collect();
        if !supporting_episode_ids.is_empty() {
            let mut episode_store = EpisodeStore::new(store);
            for record in episode_store.list_by_memory_ids(&supporting_episode_ids)? {
                let memory = Self::episode_record_to_memory(record);
                hits.push(self.hit_from_memory(&memory, query, now));
            }
        }

        if let Some(entity) = query.entity.as_deref() {
            let mut episode_store = EpisodeStore::new(store);
            for memory in episode_store
                .list_recent_memories(query.top_k.saturating_mul(3).max(6))?
                .into_iter()
                .filter(|memory| memory.entity == entity)
                .filter(|memory| {
                    query
                        .slot
                        .as_deref()
                        .is_none_or(|slot| memory.slot == slot)
                })
            {
                hits.push(self.hit_from_memory(&memory, query, now));
            }
        }

        let context_terms = Self::context_terms(query, &goal_memories);
        let mut procedure_store = ProcedureStore::new(store);
        for memory in procedure_store.list_all_memories()? {
            if !Self::procedure_matches_context(&memory, &context_terms) {
                continue;
            }
            if memory
                .to_procedure_record()
                .is_some_and(|record| Self::effective_procedure_status(&record) == ProcedureStatus::Retired)
            {
                continue;
            }
            hits.push(self.hit_from_memory(&memory, query, now));
        }

        Ok(hits)
    }

    fn episodic_hits<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<RetrievalHit>> {
        let mut episode_store = EpisodeStore::new(store);
        let episodes = episode_store
            .list_recent_memories(query.top_k.saturating_mul(3).max(6))?
            .into_iter()
            .filter(|memory| {
                query
                    .entity
                    .as_deref()
                    .is_none_or(|entity| memory.entity == entity)
            })
            .filter(|memory| {
                query
                    .slot
                    .as_deref()
                    .is_none_or(|slot| memory.slot == slot)
            })
            .map(|memory| self.hit_from_memory(&memory, query, now))
            .collect();
        Ok(episodes)
    }

    fn hit_from_memory(
        &self,
        memory: &DurableMemory,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
    ) -> RetrievalHit {
        let retention = self.retention.evaluate(memory, now);
        let mut metadata = memory.metadata.clone();
        metadata.insert(
            "memory_layer".to_string(),
            memory.memory_layer().as_str().to_string(),
        );
        if let Some(record) = memory.to_procedure_record() {
            metadata.insert(
                "procedure_status".to_string(),
                Self::effective_procedure_status(&record).as_str().to_string(),
            );
        }

        RetrievalHit {
            memory_id: Some(memory.memory_id.clone()),
            belief_id: None,
            entity: Some(memory.entity.clone()),
            slot: Some(memory.slot.clone()),
            value: Some(memory.value.clone()),
            text: memory.raw_text.clone(),
            memory_layer: Some(memory.memory_layer()),
            memory_type: Some(memory.memory_type),
            score: memory.confidence
                + retention.decayed_salience
                + Self::query_alignment_score(memory, query),
            timestamp: memory.event_timestamp(),
            scope: Some(memory.scope),
            source: Some(memory.source.source_type),
            from_belief: false,
            expired: retention.expired,
            metadata,
        }
    }

    fn filter_and_dedup_hits(
        &self,
        hits: Vec<RetrievalHit>,
        query: &RetrievalQuery,
    ) -> Vec<RetrievalHit> {
        let mut seen = HashSet::new();
        let mut filtered = Vec::new();

        for hit in hits {
            if !Self::hit_matches_query(&hit, query) {
                continue;
            }

            let key = hit
                .belief_id
                .as_ref()
                .map(|belief_id| format!("belief:{belief_id}"))
                .or_else(|| hit.memory_id.as_ref().map(|memory_id| format!("memory:{memory_id}")));
            if let Some(key) = key {
                if !seen.insert(key) {
                    continue;
                }
            }

            filtered.push(hit);
        }

        filtered
    }

    fn hit_matches_query(hit: &RetrievalHit, query: &RetrievalQuery) -> bool {
        let memory_layer = hit
            .memory_layer
            .or_else(|| hit.memory_type.map(super::enums::MemoryType::memory_layer));
        let strict_entity_match = !matches!(memory_layer, Some(MemoryLayer::Procedure));
        let strict_slot_match = !matches!(
            (query.intent, memory_layer),
            (QueryIntent::TaskState, Some(MemoryLayer::Episode | MemoryLayer::Procedure))
                | (QueryIntent::EpisodicRecall, Some(MemoryLayer::Episode))
        );

        if let Some(scope) = query.scope
            && hit.scope.is_some_and(|hit_scope| hit_scope != scope)
        {
            return false;
        }
        if let Some(entity) = query.entity.as_deref()
            && strict_entity_match
            && hit.entity.as_deref() != Some(entity)
        {
            return false;
        }
        if let Some(slot) = query.slot.as_deref()
            && strict_slot_match
            && hit.slot.as_deref() != Some(slot)
        {
            return false;
        }
        true
    }

    fn query_alignment_score(memory: &DurableMemory, query: &RetrievalQuery) -> f32 {
        let haystack = format!(
            "{} {} {} {}",
            memory.entity, memory.slot, memory.value, memory.raw_text
        );
        let lexical = Self::lexical_overlap(&haystack, &query.query_text);
        let slot_bonus = query
            .slot
            .as_deref()
            .is_some_and(|slot| memory.slot == slot) as u8 as f32
            * 0.45;
        let entity_bonus = query
            .entity
            .as_deref()
            .is_some_and(|entity| memory.entity == entity) as u8 as f32
            * 0.2;
        lexical + slot_bonus + entity_bonus
    }

    fn lexical_overlap(haystack: &str, query_text: &str) -> f32 {
        let haystack = haystack.to_lowercase();
        let normalized_query = query_text.to_lowercase();
        let tokens: Vec<_> = normalized_query
            .split_whitespace()
            .filter(|token| !token.is_empty())
            .collect();
        if tokens.is_empty() {
            return 0.0;
        }
        let matches = tokens
            .iter()
            .filter(|token| haystack.contains(**token))
            .count();
        matches as f32 / tokens.len() as f32
    }

    fn supporting_episode_ids(memory: &DurableMemory) -> Vec<String> {
        memory
            .metadata
            .get("supporting_episode_ids")
            .map(|value| {
                value
                    .split(',')
                    .filter(|entry| !entry.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn context_terms(query: &RetrievalQuery, goal_memories: &[DurableMemory]) -> HashSet<String> {
        let mut terms = HashSet::new();

        if let Some(entity) = query.entity.as_deref() {
            terms.insert(entity.to_lowercase());
        }
        if let Some(slot) = query.slot.as_deref() {
            terms.insert(slot.to_lowercase());
        }
        for token in query.query_text.split_whitespace() {
            let normalized = token
                .trim_matches(|character: char| !character.is_alphanumeric())
                .to_lowercase();
            if normalized.len() >= 3 {
                terms.insert(normalized);
            }
        }
        for memory in goal_memories {
            if let Some(workflow_key) = memory.metadata.get("workflow_key") {
                terms.insert(workflow_key.to_lowercase());
            }
            for tag in &memory.tags {
                terms.insert(tag.to_lowercase());
            }
            terms.insert(memory.slot.to_lowercase());
            terms.insert(memory.value.to_lowercase());
        }

        terms
    }

    fn procedure_matches_context(memory: &DurableMemory, context_terms: &HashSet<String>) -> bool {
        let Some(record) = memory.to_procedure_record() else {
            return false;
        };

        let workflow_key = record
            .metadata
            .get("workflow_key")
            .map(|value| value.to_lowercase());
        if workflow_key
            .as_ref()
            .is_some_and(|workflow_key| context_terms.contains(workflow_key))
        {
            return true;
        }

        if record
            .context_tags
            .iter()
            .map(|tag| tag.to_lowercase())
            .any(|tag| context_terms.contains(&tag))
        {
            return true;
        }

        let searchable = format!(
            "{} {} {}",
            record.name,
            record.description,
            record.context_tags.join(" ")
        );
        Self::lexical_overlap(&searchable, &context_terms.iter().cloned().collect::<Vec<_>>().join(" ")) > 0.0
    }

    fn effective_procedure_status(record: &ProcedureRecord) -> ProcedureStatus {
        if record.status == ProcedureStatus::Retired {
            return ProcedureStatus::Retired;
        }
        if record.status == ProcedureStatus::CoolingDown {
            return ProcedureStatus::CoolingDown;
        }

        let total = record.success_count + record.failure_count;
        if total >= 5 && record.failure_count >= record.success_count.saturating_add(3) {
            ProcedureStatus::Retired
        } else if total >= 3 && record.failure_count > record.success_count {
            ProcedureStatus::CoolingDown
        } else {
            ProcedureStatus::Active
        }
    }

    fn is_retired_procedure_hit(hit: &RetrievalHit) -> bool {
        hit.memory_layer == Some(MemoryLayer::Procedure)
            && hit
                .metadata
                .get("procedure_status")
                .and_then(|value| ProcedureStatus::from_str(value))
                == Some(ProcedureStatus::Retired)
    }

    fn episode_record_to_memory(record: super::schemas::EpisodeRecord) -> DurableMemory {
        DurableMemory {
            memory_id: record.memory_id,
            candidate_id: record.candidate_id,
            stored_at: record.stored_at,
            entity: record.entity,
            slot: record.slot,
            value: record.value,
            raw_text: record.raw_text,
            memory_type: super::enums::MemoryType::Episode,
            confidence: record.confidence,
            salience: record.salience,
            scope: record.scope,
            ttl: None,
            source: record.source,
            event_at: Some(record.event_at),
            valid_from: None,
            valid_to: None,
            internal_layer: Some(MemoryLayer::Episode),
            tags: record.tags,
            metadata: record.metadata,
            is_retraction: false,
        }
    }
}
