use std::collections::HashSet;

use super::adapters::memvid_store::MemoryStore;
use super::clock::Clock;
use super::enums::{
    BeliefStatus, BeliefViewStatus, MemoryLayer, MemoryType, ProcedureStatus, QueryIntent, Scope,
    SourceType,
};
use super::episode_store::EpisodeStore;
use super::errors::Result;
use super::goal_state_store::GoalStateStore;
use super::procedure_store::{ProcedureStore, effective_procedure_status};
use super::ranker::{
    Ranker, SCORE_SIGNAL_CONTENT_MATCH_KEY, SCORE_SIGNAL_CONTRADICTION_KEY,
    SCORE_SIGNAL_EVIDENCE_STRENGTH_KEY, SCORE_SIGNAL_GOAL_RELEVANCE_KEY,
    SCORE_SIGNAL_PROCEDURE_SUCCESS_KEY, SCORE_SIGNAL_SALIENCE_KEY, SCORE_SIGNAL_SELF_RELEVANCE_KEY,
};
use super::retention::RetentionManager;
use super::schemas::{BeliefRecord, DurableMemory, ProcedureRecord, RetrievalHit, RetrievalQuery};
use super::self_model_store::SelfModelStore;

const TASK_CONTEXT_MATCH_KEY: &str = "task_context_match";
const RETRIEVAL_ROLE_KEY: &str = "retrieval_role";
const CURRENT_FACT_SUPPORT_LIMIT: usize = 3;
const PREFERENCE_SUPPORT_LIMIT: usize = 3;
const TASK_STATE_EPISODE_LIMIT: usize = 3;
const TASK_STATE_PROCEDURE_LIMIT: usize = 2;
const HISTORY_EPISODE_LIMIT: usize = 4;
const HISTORY_STATE_LIMIT: usize = 3;
const PROCEDURE_HELP_PROCEDURE_LIMIT: usize = 3;
const PROCEDURE_HELP_EPISODE_LIMIT: usize = 3;
const FALLBACK_SEARCH_LIMIT: usize = 6;

#[derive(Debug, Default)]
struct TaskContext {
    workflow_keys: HashSet<String>,
    goal_slots: HashSet<String>,
    goal_tags: HashSet<String>,
    supporting_episode_ids: HashSet<String>,
    normalized_query: String,
}

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
        let hits = match query.intent {
            QueryIntent::CurrentFact => self.current_fact_hits(store, query, now)?,
            QueryIntent::HistoricalFact => self.historical_hits(store, query, now)?,
            QueryIntent::PreferenceLookup => self.preference_lookup_hits(store, query, now)?,
            QueryIntent::TaskState => self.task_state_hits(store, query, now)?,
            QueryIntent::EpisodicRecall => {
                if Self::is_procedure_lifecycle_query(query) {
                    self.procedure_lifecycle_history_hits(store, query)?
                } else {
                    let mut hits = self.episodic_hits(store, query, now)?;
                    if hits.len() < query.top_k {
                        hits.extend(self.search_hits(store, query, "archive_fallback", now)?);
                    }
                    hits
                }
            }
            QueryIntent::SemanticBackground => {
                if Self::is_procedure_lifecycle_query(query) {
                    self.procedure_lifecycle_history_hits(store, query)?
                } else if Self::is_procedural_help_query(query) {
                    self.procedural_help_hits(store, query, now)?
                } else {
                    self.search_hits(store, query, "archive_fallback", now)?
                }
            }
        };

        let mut filtered = self.filter_hits(hits, query);
        if !query.include_expired {
            filtered.retain(|hit| !hit.expired);
        }
        if query.intent == QueryIntent::TaskState {
            filtered.retain(|hit| !Self::is_retired_procedure_hit(hit));
        }

        let policy_profile = self.retention.policy_profile();
        let ranked = self.ranker.rerank_with_weights(
            filtered,
            query.intent,
            now,
            policy_profile.soft_weights(),
        );
        let mut deduped = self.dedup_hits(ranked);
        deduped.truncate(query.top_k);
        Ok(deduped)
    }

    fn current_fact_hits<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<RetrievalHit>> {
        let mut hits = Vec::new();

        if let (Some(entity), Some(slot)) = (query.entity.as_deref(), query.slot.as_deref()) {
            if let Some(belief) = store.get_active_belief(entity, slot)? {
                hits.push(self.hit_from_belief(&belief, query, now));
                hits.extend(self.current_fact_support_hits(store, query, now, &belief)?);
            } else if let Some(belief) = store.get_current_belief(entity, slot)?
                && belief.view_status() == BeliefViewStatus::Contested
            {
                hits.push(self.hit_from_belief(&belief, query, now));
                hits.extend(self.current_fact_support_hits(store, query, now, &belief)?);
            }
        }

        if hits.len() < query.top_k {
            hits.extend(self.search_hits(store, query, "archive_fallback", now)?);
        }
        Ok(hits)
    }

    fn current_fact_support_hits<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
        belief: &BeliefRecord,
    ) -> Result<Vec<RetrievalHit>> {
        let mut hits = Vec::new();
        let related_memories: Vec<_> = store
            .list_memories_for_belief(&belief.entity, &belief.slot)?
            .into_iter()
            .collect();
        let mut supporting_memories: Vec<_> = related_memories
            .iter()
            .filter(|memory| !memory.is_retraction)
            .filter(|memory| memory.value == belief.current_value)
            .cloned()
            .collect();
        supporting_memories
            .sort_by(|left, right| right.event_timestamp().cmp(&left.event_timestamp()));

        for memory in supporting_memories.iter().take(CURRENT_FACT_SUPPORT_LIMIT) {
            let mut hit = self.hit_from_memory_with_role(memory, query, now, "support_evidence");
            hit.metadata
                .insert("belief_relation".to_string(), "supporting".to_string());
            hits.push(hit);
        }

        let supporting_episode_ids = Self::collect_supporting_episode_ids(&supporting_memories);
        if !supporting_episode_ids.is_empty() {
            let mut episode_store = EpisodeStore::new(store);
            for record in episode_store
                .list_by_memory_ids(&supporting_episode_ids)?
                .into_iter()
                .take(CURRENT_FACT_SUPPORT_LIMIT)
            {
                let memory = Self::episode_record_to_memory(record);
                let mut hit =
                    self.hit_from_memory_with_role(&memory, query, now, "support_evidence");
                hit.metadata
                    .insert("belief_relation".to_string(), "supporting".to_string());
                hits.push(hit);
            }
        }

        if belief.status == BeliefStatus::Disputed {
            let mut opposing_memories: Vec<_> = related_memories
                .into_iter()
                .filter(|memory| belief.opposing_memory_ids.contains(&memory.memory_id))
                .collect();
            opposing_memories
                .sort_by(|left, right| right.event_timestamp().cmp(&left.event_timestamp()));
            for memory in opposing_memories.iter().take(CURRENT_FACT_SUPPORT_LIMIT) {
                let mut hit =
                    self.hit_from_memory_with_role(memory, query, now, "support_evidence");
                hit.metadata
                    .insert("belief_relation".to_string(), "opposing".to_string());
                hits.push(hit);
            }
        }

        Ok(hits)
    }

    fn historical_hits<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<RetrievalHit>> {
        let mut hits = Vec::new();

        if let (Some(entity), Some(slot), Some(as_of)) =
            (query.entity.as_deref(), query.slot.as_deref(), query.as_of)
        {
            let mut episode_store = EpisodeStore::new(store);
            let mut episodes: Vec<_> = episode_store
                .list_recent_memories(query.top_k.saturating_mul(4).max(HISTORY_EPISODE_LIMIT))?
                .into_iter()
                .filter(|memory| memory.entity == entity)
                .filter(|memory| memory.slot == slot)
                .filter(|memory| memory.event_timestamp() <= as_of)
                .collect();
            episodes.sort_by(|left, right| right.event_timestamp().cmp(&left.event_timestamp()));
            for memory in episodes.into_iter().take(HISTORY_EPISODE_LIMIT) {
                hits.push(self.hit_from_memory_with_role(&memory, query, now, "direct_answer"));
            }

            let mut state_memories: Vec<_> = store
                .list_memories_for_belief(entity, slot)?
                .into_iter()
                .filter(|memory| memory.event_timestamp() <= as_of)
                .collect();
            state_memories
                .sort_by(|left, right| right.event_timestamp().cmp(&left.event_timestamp()));
            for memory in state_memories.into_iter().take(HISTORY_STATE_LIMIT) {
                hits.push(self.hit_from_memory_with_role(&memory, query, now, "support_evidence"));
            }
        }

        if hits.len() < query.top_k {
            hits.extend(self.search_hits(store, query, "archive_fallback", now)?);
        }
        Ok(hits)
    }

    fn preference_lookup_hits<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<RetrievalHit>> {
        let direct_memories = self.preference_direct_memories(store, query)?;
        let direct_ids: HashSet<_> = direct_memories
            .iter()
            .map(|memory| memory.memory_id.clone())
            .collect();
        let mut hits: Vec<_> = direct_memories
            .iter()
            .map(|memory| self.hit_from_memory_with_role(memory, query, now, "direct_answer"))
            .collect();
        hits.extend(self.preference_support_hits(store, query, now, &direct_ids)?);

        if hits.len() < query.top_k {
            hits.extend(self.search_hits(store, query, "archive_fallback", now)?);
        }
        Ok(hits)
    }

    fn preference_direct_memories<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
    ) -> Result<Vec<DurableMemory>> {
        let Some(entity) = query.entity.as_deref() else {
            return Ok(Vec::new());
        };

        let memories = {
            let mut self_model_store = SelfModelStore::new(store);
            self_model_store.list_for_entity_memories(entity)?
        };
        if let Some(slot) = query.slot.as_deref() {
            Ok(memories
                .into_iter()
                .find(|memory| memory.slot == slot)
                .into_iter()
                .collect())
        } else {
            let mut seen_slots = HashSet::new();
            Ok(memories
                .into_iter()
                .filter(|memory| seen_slots.insert(memory.slot.clone()))
                .collect())
        }
    }

    fn preference_support_hits<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
        direct_ids: &HashSet<String>,
    ) -> Result<Vec<RetrievalHit>> {
        let Some(entity) = query.entity.as_deref() else {
            return Ok(Vec::new());
        };

        let memories = {
            let mut self_model_store = SelfModelStore::new(store);
            self_model_store.list_for_entity_memories(entity)?
        };
        let mut support_memories: Vec<_> = memories
            .into_iter()
            .filter(|memory| !direct_ids.contains(&memory.memory_id))
            .filter(|memory| query.slot.as_deref().is_none_or(|slot| memory.slot == slot))
            .collect();
        support_memories
            .sort_by(|left, right| right.event_timestamp().cmp(&left.event_timestamp()));

        let mut hits = Vec::new();
        for memory in support_memories.iter().take(PREFERENCE_SUPPORT_LIMIT) {
            hits.push(self.hit_from_memory_with_role(memory, query, now, "support_evidence"));
        }

        let supporting_episode_ids = Self::collect_supporting_episode_ids(&support_memories);
        if !supporting_episode_ids.is_empty() {
            let mut episode_store = EpisodeStore::new(store);
            for record in episode_store
                .list_by_memory_ids(&supporting_episode_ids)?
                .into_iter()
                .take(PREFERENCE_SUPPORT_LIMIT)
            {
                let memory = Self::episode_record_to_memory(record);
                hits.push(self.hit_from_memory_with_role(&memory, query, now, "support_evidence"));
            }
        }

        Ok(hits)
    }

    fn task_state_hits<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<RetrievalHit>> {
        let mut hits = Vec::new();

        let mut goal_memories: Vec<_> = {
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
                .filter(|memory| query.slot.as_deref().is_none_or(|slot| memory.slot == slot))
                .collect()
        };
        goal_memories
            .sort_by(|left, right| Self::goal_sort_key(right).cmp(&Self::goal_sort_key(left)));
        for memory in &goal_memories {
            hits.push(self.hit_from_memory_with_role(memory, query, now, "direct_answer"));
        }

        let task_context = Self::task_context(query, &goal_memories);
        hits.extend(self.task_state_episode_hits(store, query, now, &task_context)?);
        hits.extend(self.task_state_procedure_hits(store, query, now, &task_context)?);

        if hits.len() < query.top_k {
            hits.extend(self.search_hits(store, query, "archive_fallback", now)?);
        }
        Ok(hits)
    }

    fn task_state_episode_hits<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
        task_context: &TaskContext,
    ) -> Result<Vec<RetrievalHit>> {
        let mut hits = Vec::new();
        let supporting_episode_ids: Vec<_> = task_context
            .supporting_episode_ids
            .iter()
            .cloned()
            .collect();
        if !supporting_episode_ids.is_empty() {
            let mut episode_store = EpisodeStore::new(store);
            for record in episode_store
                .list_by_memory_ids(&supporting_episode_ids)?
                .into_iter()
                .take(TASK_STATE_EPISODE_LIMIT)
            {
                let memory = Self::episode_record_to_memory(record);
                hits.push(self.hit_from_memory_with_task_context(
                    &memory,
                    query,
                    now,
                    "support_evidence",
                    "supporting_episode",
                ));
            }
        }

        let mut episode_store = EpisodeStore::new(store);
        for memory in episode_store
            .list_recent_memories(query.top_k.saturating_mul(4).max(TASK_STATE_EPISODE_LIMIT))?
            .into_iter()
            .filter(|memory| {
                query
                    .entity
                    .as_deref()
                    .is_none_or(|entity| memory.entity == entity)
            })
            .filter(|memory| Self::episode_matches_task_context(memory, task_context, query))
            .take(TASK_STATE_EPISODE_LIMIT)
        {
            hits.push(self.hit_from_memory_with_task_context(
                &memory,
                query,
                now,
                "support_evidence",
                "aligned_episode",
            ));
        }

        Ok(hits)
    }

    fn task_state_procedure_hits<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
        task_context: &TaskContext,
    ) -> Result<Vec<RetrievalHit>> {
        let mut procedure_store = ProcedureStore::new(store);
        let mut memories: Vec<_> = procedure_store
            .list_all_memories()?
            .into_iter()
            .filter(|memory| Self::procedure_matches_task_context(memory, task_context, query))
            .filter(|memory| {
                memory.to_procedure_record().is_none_or(|record| {
                    effective_procedure_status(&record) != ProcedureStatus::Retired
                })
            })
            .collect();
        memories.sort_by(|left, right| {
            Self::procedure_rank(right)
                .cmp(&Self::procedure_rank(left))
                .then_with(|| right.event_timestamp().cmp(&left.event_timestamp()))
        });

        let mut hits = Vec::new();
        for memory in memories.into_iter().take(TASK_STATE_PROCEDURE_LIMIT) {
            hits.push(self.hit_from_memory_with_task_context(
                &memory,
                query,
                now,
                "support_evidence",
                "aligned_procedure",
            ));
        }
        Ok(hits)
    }

    fn procedural_help_hits<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<RetrievalHit>> {
        let task_context = Self::task_context(query, &[]);
        let direct_procedures: Vec<_> = {
            let mut procedure_store = ProcedureStore::new(store);
            procedure_store
                .list_all_memories()?
                .into_iter()
                .filter(|memory| Self::procedure_matches_task_context(memory, &task_context, query))
                .filter(|memory| {
                    memory.to_procedure_record().is_none_or(|record| {
                        effective_procedure_status(&record) != ProcedureStatus::Retired
                    })
                })
                .take(PROCEDURE_HELP_PROCEDURE_LIMIT)
                .collect::<Vec<_>>()
        };

        let mut hits: Vec<_> = direct_procedures
            .iter()
            .map(|memory| self.hit_from_memory_with_role(memory, query, now, "direct_answer"))
            .collect();
        let workflow_keys: HashSet<_> = direct_procedures
            .iter()
            .filter_map(|memory| memory.metadata.get("workflow_key").cloned())
            .map(|key| key.to_lowercase())
            .collect();

        let mut episode_store = EpisodeStore::new(store);
        for memory in episode_store
            .list_recent_memories(
                query
                    .top_k
                    .saturating_mul(4)
                    .max(PROCEDURE_HELP_EPISODE_LIMIT),
            )?
            .into_iter()
            .filter(|memory| {
                Self::is_success_outcome(memory.metadata.get("outcome").map(String::as_str))
            })
            .filter(|memory| Self::episode_matches_procedural_help(memory, query, &workflow_keys))
            .take(PROCEDURE_HELP_EPISODE_LIMIT)
        {
            hits.push(self.hit_from_memory_with_role(&memory, query, now, "support_evidence"));
        }

        if hits.len() < query.top_k {
            hits.extend(self.search_hits(store, query, "archive_fallback", now)?);
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
            .list_recent_memories(query.top_k.saturating_mul(4).max(6))?
            .into_iter()
            .filter(|memory| {
                query
                    .entity
                    .as_deref()
                    .is_none_or(|entity| memory.entity == entity)
            })
            .filter(|memory| query.slot.as_deref().is_none_or(|slot| memory.slot == slot))
            .map(|memory| self.hit_from_memory_with_role(&memory, query, now, "direct_answer"))
            .collect();
        Ok(episodes)
    }

    fn procedure_lifecycle_history_hits<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
    ) -> Result<Vec<RetrievalHit>> {
        let mut hits: Vec<_> = store
            .search(query)?
            .into_iter()
            .filter(|hit| hit.memory_layer == Some(MemoryLayer::Trace))
            .filter(|hit| {
                hit.metadata.get("action").map(String::as_str) == Some("procedure_status_changed")
            })
            .collect();
        for hit in &mut hits {
            hit.score += 0.75;
            hit.metadata
                .insert(RETRIEVAL_ROLE_KEY.to_string(), "direct_answer".to_string());
        }
        Ok(hits)
    }

    fn search_hits<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        role: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<RetrievalHit>> {
        let mut hits = store.search(query)?;
        for hit in &mut hits {
            hit.metadata
                .insert(RETRIEVAL_ROLE_KEY.to_string(), role.to_string());
            self.annotate_search_hit_signals(hit, query, role, now);
        }
        hits.truncate(FALLBACK_SEARCH_LIMIT);
        Ok(hits)
    }

    fn hit_from_belief(
        &self,
        belief: &BeliefRecord,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
    ) -> RetrievalHit {
        let mut metadata = std::collections::BTreeMap::from([
            (RETRIEVAL_ROLE_KEY.to_string(), "direct_answer".to_string()),
            (
                "supporting_memory_count".to_string(),
                belief.supporting_memory_ids.len().to_string(),
            ),
            (
                "opposing_memory_count".to_string(),
                belief.opposing_memory_ids.len().to_string(),
            ),
            (
                "belief_status".to_string(),
                belief.status.as_str().to_string(),
            ),
            (
                "belief_retrieval_status".to_string(),
                belief.view_status().as_str().to_string(),
            ),
            (
                "contradictions_observed".to_string(),
                belief.contradictions_observed.to_string(),
            ),
        ]);
        if !belief.opposing_memory_ids.is_empty() {
            metadata.insert(
                "opposing_memory_ids".to_string(),
                belief.opposing_memory_ids.join(","),
            );
        }
        if let Some(last_contradiction_at) = belief.last_contradiction_at {
            metadata.insert(
                "last_contradiction_at".to_string(),
                last_contradiction_at.to_rfc3339(),
            );
        }
        if let Some(seconds) = belief.time_in_dispute_seconds(now) {
            metadata.insert("time_in_dispute_seconds".to_string(), seconds.to_string());
        }
        if let Some(seconds) = belief.time_to_last_resolution_seconds {
            metadata.insert(
                "time_to_last_resolution_seconds".to_string(),
                seconds.to_string(),
            );
        }
        if belief.positive_outcome_count > 0 {
            metadata.insert(
                "positive_outcome_count".to_string(),
                belief.positive_outcome_count.to_string(),
            );
        }
        if belief.negative_outcome_count > 0 {
            metadata.insert(
                "negative_outcome_count".to_string(),
                belief.negative_outcome_count.to_string(),
            );
        }
        if let Some(last_outcome_at) = belief.last_outcome_at {
            metadata.insert("last_outcome_at".to_string(), last_outcome_at.to_rfc3339());
            metadata.insert(
                "outcome_impact_score".to_string(),
                format!("{:.6}", belief.outcome_impact_score()),
            );
        }
        Self::set_score_signal(
            &mut metadata,
            SCORE_SIGNAL_CONTENT_MATCH_KEY,
            Self::belief_alignment_score(belief, query),
        );
        Self::set_score_signal(
            &mut metadata,
            SCORE_SIGNAL_EVIDENCE_STRENGTH_KEY,
            belief.effective_confidence(now),
        );
        Self::set_score_signal(
            &mut metadata,
            SCORE_SIGNAL_CONTRADICTION_KEY,
            match belief.view_status() {
                BeliefViewStatus::Active => 0.0,
                BeliefViewStatus::Contested => 1.0,
                BeliefViewStatus::Superseded => 1.6,
                BeliefViewStatus::Retracted => 2.0,
            },
        );
        Self::set_score_signal(&mut metadata, SCORE_SIGNAL_GOAL_RELEVANCE_KEY, 0.0);
        Self::set_score_signal(&mut metadata, SCORE_SIGNAL_SELF_RELEVANCE_KEY, 0.0);
        Self::set_score_signal(&mut metadata, SCORE_SIGNAL_SALIENCE_KEY, 0.0);
        Self::set_score_signal(&mut metadata, SCORE_SIGNAL_PROCEDURE_SUCCESS_KEY, 0.0);

        RetrievalHit {
            memory_id: belief.supporting_memory_ids.last().cloned(),
            belief_id: Some(belief.belief_id.clone()),
            entity: Some(belief.entity.clone()),
            slot: Some(belief.slot.clone()),
            value: Some(belief.current_value.clone()),
            text: belief.current_value.clone(),
            memory_layer: Some(MemoryLayer::Belief),
            memory_type: Some(MemoryType::Fact),
            score: belief.effective_confidence(now),
            timestamp: belief.last_reviewed_at,
            scope: query.scope,
            source: None,
            from_belief: true,
            expired: false,
            metadata,
        }
    }

    fn hit_from_memory_with_role(
        &self,
        memory: &DurableMemory,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
        role: &str,
    ) -> RetrievalHit {
        let retention = self.retention.evaluate(memory, now);
        let mut metadata = memory.metadata.clone();
        metadata.insert(
            "memory_layer".to_string(),
            memory.memory_layer().as_str().to_string(),
        );
        metadata.insert(RETRIEVAL_ROLE_KEY.to_string(), role.to_string());
        metadata.insert("source_id".to_string(), memory.source.source_id.clone());
        metadata.insert(
            "source_type".to_string(),
            format!("{:?}", memory.source.source_type).to_lowercase(),
        );
        metadata.insert(
            "source_weight".to_string(),
            memory.source.trust_weight.to_string(),
        );
        metadata.insert("confidence".to_string(), memory.confidence.to_string());
        metadata.insert(
            "salience".to_string(),
            retention.decayed_salience.to_string(),
        );
        metadata.insert("stored_at".to_string(), memory.stored_at.to_rfc3339());
        metadata.insert(
            "updated_at".to_string(),
            memory.version_timestamp().to_rfc3339(),
        );
        metadata.insert(
            "event_at".to_string(),
            memory.event_timestamp().to_rfc3339(),
        );
        if let Some(record) = memory.to_procedure_record() {
            metadata.insert(
                "procedure_status".to_string(),
                effective_procedure_status(&record).as_str().to_string(),
            );
        }
        let goal_relevance =
            Self::goal_relevance_signal(memory.memory_layer(), &metadata, query.intent);
        let procedure_success =
            Self::procedure_success_signal(memory.to_procedure_record().as_ref());
        Self::set_score_signal(
            &mut metadata,
            SCORE_SIGNAL_CONTENT_MATCH_KEY,
            Self::query_alignment_score(memory, query),
        );
        Self::set_score_signal(
            &mut metadata,
            SCORE_SIGNAL_SALIENCE_KEY,
            retention.decayed_salience,
        );
        Self::set_score_signal(
            &mut metadata,
            SCORE_SIGNAL_EVIDENCE_STRENGTH_KEY,
            memory.confidence,
        );
        Self::set_score_signal(
            &mut metadata,
            SCORE_SIGNAL_GOAL_RELEVANCE_KEY,
            goal_relevance,
        );
        Self::set_score_signal(
            &mut metadata,
            SCORE_SIGNAL_SELF_RELEVANCE_KEY,
            Self::self_relevance_signal(memory.memory_layer(), query.intent, role),
        );
        Self::set_score_signal(
            &mut metadata,
            SCORE_SIGNAL_PROCEDURE_SUCCESS_KEY,
            procedure_success,
        );
        Self::set_score_signal(&mut metadata, SCORE_SIGNAL_CONTRADICTION_KEY, 0.0);

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

    fn hit_from_memory_with_task_context(
        &self,
        memory: &DurableMemory,
        query: &RetrievalQuery,
        now: chrono::DateTime<chrono::Utc>,
        role: &str,
        context_match: &str,
    ) -> RetrievalHit {
        let mut hit = self.hit_from_memory_with_role(memory, query, now, role);
        hit.metadata.insert(
            TASK_CONTEXT_MATCH_KEY.to_string(),
            context_match.to_string(),
        );
        Self::set_score_signal(
            &mut hit.metadata,
            SCORE_SIGNAL_GOAL_RELEVANCE_KEY,
            Self::goal_relevance_signal_from_context(context_match),
        );
        hit
    }

    fn filter_hits(&self, hits: Vec<RetrievalHit>, query: &RetrievalQuery) -> Vec<RetrievalHit> {
        hits.into_iter()
            .filter(|hit| Self::hit_matches_query(hit, query))
            .collect()
    }

    fn dedup_hits(&self, hits: Vec<RetrievalHit>) -> Vec<RetrievalHit> {
        let mut seen = HashSet::new();
        let mut deduped = Vec::new();
        for hit in hits {
            let key = Self::dedup_key(&hit);
            if seen.insert(key) {
                deduped.push(hit);
            }
        }
        deduped
    }

    fn dedup_key(hit: &RetrievalHit) -> String {
        let entity = hit.entity.as_deref().unwrap_or("").trim().to_lowercase();
        let slot = hit.slot.as_deref().unwrap_or("").trim().to_lowercase();
        let value = hit.value.as_deref().unwrap_or("").trim().to_lowercase();
        let layer = hit
            .memory_layer
            .or_else(|| hit.memory_type.map(MemoryType::memory_layer))
            .map(MemoryLayer::as_str)
            .unwrap_or("unknown");
        let workflow_key = hit
            .metadata
            .get("workflow_key")
            .map(|value| value.trim().to_lowercase())
            .unwrap_or_default();
        let source_id = hit
            .metadata
            .get("source_id")
            .map(|value| value.trim().to_lowercase())
            .or_else(|| hit.memory_id.as_ref().map(|value| value.to_lowercase()))
            .unwrap_or_default();
        let bucket = hit.timestamp.timestamp().div_euclid(86_400);

        if !entity.is_empty() || !slot.is_empty() || !value.is_empty() || !workflow_key.is_empty() {
            return format!(
                "semantic|{layer}|{entity}|{slot}|{value}|{workflow_key}|{source_id}|{bucket}"
            );
        }

        if let Some(belief_id) = hit.belief_id.as_deref() {
            return format!("belief|{belief_id}");
        }
        if let Some(memory_id) = hit.memory_id.as_deref() {
            return format!("memory|{memory_id}");
        }

        let normalized_text = hit
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        format!(
            "text|{}|{}|{bucket}",
            hit.memory_layer
                .map(MemoryLayer::as_str)
                .unwrap_or("unknown"),
            normalized_text
        )
    }

    fn hit_matches_query(hit: &RetrievalHit, query: &RetrievalQuery) -> bool {
        let memory_layer = hit
            .memory_layer
            .or_else(|| hit.memory_type.map(super::enums::MemoryType::memory_layer));
        let has_task_context_match = hit.metadata.contains_key(TASK_CONTEXT_MATCH_KEY);
        let strict_entity_match =
            !matches!(memory_layer, Some(MemoryLayer::Procedure)) || !has_task_context_match;
        let strict_slot_match = !matches!(
            (query.intent, memory_layer),
            (
                QueryIntent::TaskState,
                Some(MemoryLayer::Episode | MemoryLayer::Procedure)
            ) | (QueryIntent::EpisodicRecall, Some(MemoryLayer::Episode))
        ) || !has_task_context_match;

        if query.intent == QueryIntent::TaskState
            && matches!(
                memory_layer,
                Some(MemoryLayer::Procedure | MemoryLayer::Episode)
            )
            && !has_task_context_match
        {
            return false;
        }

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

    fn belief_alignment_score(belief: &BeliefRecord, query: &RetrievalQuery) -> f32 {
        let haystack = format!("{} {} {}", belief.entity, belief.slot, belief.current_value);
        let lexical = Self::lexical_overlap(&haystack, &query.query_text);
        let slot_bonus = query
            .slot
            .as_deref()
            .is_some_and(|slot| belief.slot == slot) as u8 as f32
            * 0.45;
        let entity_bonus = query
            .entity
            .as_deref()
            .is_some_and(|entity| belief.entity == entity) as u8 as f32
            * 0.2;
        lexical + slot_bonus + entity_bonus
    }

    fn annotate_search_hit_signals(
        &self,
        hit: &mut RetrievalHit,
        query: &RetrievalQuery,
        role: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) {
        let content_signal = Self::query_alignment_score_from_hit(hit, query).max(hit.score);
        let memory_layer = hit
            .memory_layer
            .or_else(|| hit.memory_type.map(MemoryType::memory_layer));
        let confidence = hit
            .metadata
            .get("confidence")
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(0.0);
        let salience = self.search_hit_salience(hit, now);
        let goal_relevance = memory_layer.map_or(0.0, |layer| {
            Self::goal_relevance_signal(layer, &hit.metadata, query.intent)
        });
        let self_relevance = memory_layer.map_or(0.0, |layer| {
            Self::self_relevance_signal(layer, query.intent, role)
        });
        let procedure_success = Self::procedure_success_signal_from_metadata(&hit.metadata);
        let contradiction_signal = match hit
            .metadata
            .get("belief_retrieval_status")
            .map(String::as_str)
        {
            Some("contested") => 1.0,
            Some("superseded") => 1.6,
            Some("retracted") => 2.0,
            _ => 0.0,
        };

        hit.metadata
            .insert("salience".to_string(), salience.to_string());
        Self::set_score_signal(
            &mut hit.metadata,
            SCORE_SIGNAL_CONTENT_MATCH_KEY,
            content_signal,
        );
        Self::set_score_signal(
            &mut hit.metadata,
            SCORE_SIGNAL_EVIDENCE_STRENGTH_KEY,
            confidence,
        );
        Self::set_score_signal(&mut hit.metadata, SCORE_SIGNAL_SALIENCE_KEY, salience);
        Self::set_score_signal(
            &mut hit.metadata,
            SCORE_SIGNAL_GOAL_RELEVANCE_KEY,
            goal_relevance,
        );
        Self::set_score_signal(
            &mut hit.metadata,
            SCORE_SIGNAL_SELF_RELEVANCE_KEY,
            self_relevance,
        );
        Self::set_score_signal(
            &mut hit.metadata,
            SCORE_SIGNAL_PROCEDURE_SUCCESS_KEY,
            procedure_success,
        );
        Self::set_score_signal(
            &mut hit.metadata,
            SCORE_SIGNAL_CONTRADICTION_KEY,
            contradiction_signal,
        );
    }

    fn search_hit_salience(&self, hit: &RetrievalHit, now: chrono::DateTime<chrono::Utc>) -> f32 {
        let base_salience = hit
            .metadata
            .get("salience")
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(0.0);
        let Some(memory_type) = hit.memory_type else {
            return base_salience;
        };
        let Some(memory_layer) = hit
            .memory_layer
            .or_else(|| hit.memory_type.map(MemoryType::memory_layer))
        else {
            return base_salience;
        };

        let synthetic = DurableMemory {
            memory_id: hit.memory_id.clone().unwrap_or_default(),
            candidate_id: String::new(),
            stored_at: Self::parse_hit_timestamp(hit.metadata.get("stored_at"))
                .unwrap_or(hit.timestamp),
            updated_at: Some(
                Self::parse_hit_timestamp(hit.metadata.get("updated_at"))
                    .or_else(|| Self::parse_hit_timestamp(hit.metadata.get("stored_at")))
                    .unwrap_or(hit.timestamp),
            ),
            entity: hit.entity.clone().unwrap_or_default(),
            slot: hit.slot.clone().unwrap_or_default(),
            value: hit.value.clone().unwrap_or_default(),
            raw_text: hit.text.clone(),
            memory_type,
            confidence: hit
                .metadata
                .get("confidence")
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(0.0),
            salience: base_salience,
            scope: hit.scope.unwrap_or(Scope::Private),
            ttl: hit
                .metadata
                .get("ttl")
                .and_then(|value| value.parse::<i64>().ok()),
            source: super::schemas::Provenance {
                source_type: hit.source.unwrap_or_else(|| {
                    hit.metadata
                        .get("source_type")
                        .map(String::as_str)
                        .map(Self::parse_source_type)
                        .unwrap_or(SourceType::Chat)
                }),
                source_id: hit.metadata.get("source_id").cloned().unwrap_or_default(),
                source_label: None,
                observed_by: None,
                trust_weight: hit
                    .metadata
                    .get("source_weight")
                    .and_then(|value| value.parse::<f32>().ok())
                    .unwrap_or(0.5),
            },
            event_at: Self::parse_hit_timestamp(hit.metadata.get("event_at")),
            valid_from: Self::parse_hit_timestamp(hit.metadata.get("valid_from")),
            valid_to: Self::parse_hit_timestamp(hit.metadata.get("valid_to")),
            internal_layer: Some(memory_layer),
            tags: Vec::new(),
            metadata: hit.metadata.clone(),
            is_retraction: false,
        };

        self.retention.evaluate(&synthetic, now).decayed_salience
    }

    fn parse_hit_timestamp(value: Option<&String>) -> Option<chrono::DateTime<chrono::Utc>> {
        value
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&chrono::Utc))
    }

    fn parse_source_type(value: &str) -> SourceType {
        match value {
            "file" => SourceType::File,
            "tool" => SourceType::Tool,
            "system" => SourceType::System,
            "external" => SourceType::External,
            _ => SourceType::Chat,
        }
    }

    fn query_alignment_score_from_hit(hit: &RetrievalHit, query: &RetrievalQuery) -> f32 {
        let haystack = format!(
            "{} {} {} {}",
            hit.entity.as_deref().unwrap_or(""),
            hit.slot.as_deref().unwrap_or(""),
            hit.value.as_deref().unwrap_or(""),
            hit.text,
        );
        let lexical = Self::lexical_overlap(&haystack, &query.query_text);
        let slot_bonus = query
            .slot
            .as_deref()
            .is_some_and(|slot| hit.slot.as_deref() == Some(slot)) as u8
            as f32
            * 0.45;
        let entity_bonus = query
            .entity
            .as_deref()
            .is_some_and(|entity| hit.entity.as_deref() == Some(entity))
            as u8 as f32
            * 0.2;
        lexical + slot_bonus + entity_bonus
    }

    fn goal_relevance_signal(
        memory_layer: MemoryLayer,
        metadata: &std::collections::BTreeMap<String, String>,
        intent: QueryIntent,
    ) -> f32 {
        if intent != QueryIntent::TaskState {
            return 0.0;
        }

        match memory_layer {
            MemoryLayer::GoalState => match metadata.get("goal_status").map(String::as_str) {
                Some("blocked" | "waiting_on_user" | "waiting_on_system") => 1.0,
                Some("active") => 0.7,
                Some("completed" | "inactive") => -0.5,
                _ => 0.6,
            },
            _ => 0.0,
        }
    }

    fn goal_relevance_signal_from_context(context_match: &str) -> f32 {
        match context_match {
            "supporting_episode" => 0.95,
            "aligned_episode" => 0.75,
            "aligned_procedure" => 0.7,
            _ => 0.0,
        }
    }

    fn self_relevance_signal(memory_layer: MemoryLayer, intent: QueryIntent, role: &str) -> f32 {
        if intent != QueryIntent::PreferenceLookup || memory_layer != MemoryLayer::SelfModel {
            return 0.0;
        }

        match role {
            "direct_answer" => 1.0,
            "support_evidence" => 0.6,
            "archive_fallback" => 0.4,
            _ => 0.0,
        }
    }

    fn procedure_success_signal(record: Option<&ProcedureRecord>) -> f32 {
        let Some(record) = record else {
            return 0.0;
        };
        let total_runs = record.success_count + record.failure_count;
        if total_runs == 0 {
            0.0
        } else {
            record.success_count as f32 / total_runs as f32
        }
    }

    fn procedure_success_signal_from_metadata(
        metadata: &std::collections::BTreeMap<String, String>,
    ) -> f32 {
        let success_count = metadata
            .get("success_count")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let failure_count = metadata
            .get("failure_count")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let total_runs = success_count + failure_count;
        if total_runs == 0 {
            0.0
        } else {
            success_count as f32 / total_runs as f32
        }
    }

    fn set_score_signal(
        metadata: &mut std::collections::BTreeMap<String, String>,
        key: &str,
        value: f32,
    ) {
        metadata.insert(key.to_string(), format!("{value:.6}"));
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

    fn is_procedure_lifecycle_query(query: &RetrievalQuery) -> bool {
        let lower = query.query_text.to_lowercase();
        (lower.contains("history")
            || lower.contains("lifecycle")
            || lower.contains("transition")
            || lower.contains("status change")
            || lower.contains("why did"))
            && (lower.contains("procedure")
                || lower.contains("workflow")
                || lower.contains("repo_review")
                || lower.contains("status"))
    }

    fn is_procedural_help_query(query: &RetrievalQuery) -> bool {
        let lower = query.query_text.to_lowercase();
        (lower.contains("procedure")
            || lower.contains("workflow")
            || lower.contains("steps")
            || lower.contains("runbook")
            || lower.contains("how do"))
            && !Self::is_procedure_lifecycle_query(query)
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

    fn collect_supporting_episode_ids(memories: &[DurableMemory]) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        for memory in memories {
            for episode_id in Self::supporting_episode_ids(memory) {
                if seen.insert(episode_id.clone()) {
                    ids.push(episode_id);
                }
            }
        }
        ids
    }

    fn task_context(query: &RetrievalQuery, goal_memories: &[DurableMemory]) -> TaskContext {
        let mut context = TaskContext {
            normalized_query: query.query_text.to_lowercase(),
            ..TaskContext::default()
        };

        if let Some(slot) = query.slot.as_deref() {
            context.goal_slots.insert(slot.to_lowercase());
        }
        for memory in goal_memories {
            if let Some(workflow_key) = memory.metadata.get("workflow_key") {
                context.workflow_keys.insert(workflow_key.to_lowercase());
            }
            for tag in &memory.tags {
                context.goal_tags.insert(tag.to_lowercase());
            }
            context.goal_slots.insert(memory.slot.to_lowercase());
            context
                .supporting_episode_ids
                .extend(Self::supporting_episode_ids(memory));
        }

        context
    }

    fn procedure_matches_task_context(
        memory: &DurableMemory,
        task_context: &TaskContext,
        query: &RetrievalQuery,
    ) -> bool {
        let Some(record) = memory.to_procedure_record() else {
            return false;
        };

        let workflow_key = record
            .metadata
            .get("workflow_key")
            .map(|value| value.to_lowercase());
        if workflow_key.as_ref().is_some_and(|workflow_key| {
            task_context.workflow_keys.contains(workflow_key)
                || task_context.normalized_query.contains(workflow_key)
        }) {
            return true;
        }

        if query.slot.as_deref().is_some_and(|slot| {
            slot.eq_ignore_ascii_case(&record.name)
                || record
                    .context_tags
                    .iter()
                    .any(|tag| tag.eq_ignore_ascii_case(slot))
        }) {
            return true;
        }

        record.context_tags.iter().any(|tag| {
            task_context.goal_tags.contains(&tag.to_lowercase())
                || task_context.goal_slots.contains(&tag.to_lowercase())
        })
    }

    fn episode_matches_task_context(
        memory: &DurableMemory,
        task_context: &TaskContext,
        query: &RetrievalQuery,
    ) -> bool {
        if task_context
            .supporting_episode_ids
            .contains(&memory.memory_id)
        {
            return true;
        }

        if let Some(workflow_key) = memory.metadata.get("workflow_key") {
            let workflow_key = workflow_key.to_lowercase();
            if task_context.workflow_keys.contains(&workflow_key)
                || task_context.normalized_query.contains(&workflow_key)
            {
                return true;
            }
        }

        if query
            .slot
            .as_deref()
            .is_some_and(|slot| memory.slot == slot)
        {
            return true;
        }

        memory
            .tags
            .iter()
            .any(|tag| task_context.goal_tags.contains(&tag.to_lowercase()))
    }

    fn episode_matches_procedural_help(
        memory: &DurableMemory,
        query: &RetrievalQuery,
        workflow_keys: &HashSet<String>,
    ) -> bool {
        if let Some(workflow_key) = memory.metadata.get("workflow_key")
            && (workflow_keys.contains(&workflow_key.to_lowercase())
                || query
                    .query_text
                    .to_lowercase()
                    .contains(&workflow_key.to_lowercase()))
        {
            return true;
        }

        query
            .slot
            .as_deref()
            .is_some_and(|slot| memory.slot.eq_ignore_ascii_case(slot))
            || memory.tags.iter().any(|tag| {
                query
                    .query_text
                    .to_lowercase()
                    .contains(&tag.to_lowercase())
            })
    }

    fn goal_sort_key(memory: &DurableMemory) -> (u8, chrono::DateTime<chrono::Utc>) {
        let status = memory
            .metadata
            .get("goal_status")
            .map(String::as_str)
            .unwrap_or("active");
        let priority = match status {
            "blocked" | "waiting_on_user" | "waiting_on_system" => 3,
            "active" => 2,
            "completed" => 0,
            _ => 1,
        };
        (priority, memory.event_timestamp())
    }

    fn procedure_rank(memory: &DurableMemory) -> u8 {
        memory
            .to_procedure_record()
            .map(|record| match effective_procedure_status(&record) {
                ProcedureStatus::Active => 3,
                ProcedureStatus::CoolingDown => 2,
                ProcedureStatus::Retired => 0,
            })
            .unwrap_or(1)
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
            updated_at: Some(record.stored_at),
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
