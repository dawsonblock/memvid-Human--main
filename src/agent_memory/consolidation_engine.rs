use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use super::adapters::memvid_store::MemoryStore;
use super::clock::Clock;
use super::enums::MemoryLayer;
use super::episode_store::EpisodeStore;
use super::errors::Result;
use super::goal_state_store::GoalStateStore;
use super::procedure_store::{ProcedureStatusTransition, ProcedureStore};
use super::schemas::{ConsolidationRecord, DurableMemory};
use super::self_model_store::SelfModelStore;

const CONSOLIDATION_WINDOW_DAYS: i64 = 30;
const BELIEF_STABILITY_MIN_DAYS: i64 = 3;
const BLOCKER_THRESHOLD: usize = 3;

/// Consolidation result emitted after repeated bounded patterns are promoted.
#[derive(Debug, Clone)]
pub struct ConsolidationOutcome {
    pub record: ConsolidationRecord,
    pub trace_id: String,
    pub learned_procedure_id: Option<String>,
    pub procedure_status_transition: Option<ProcedureStatusTransition>,
}

/// Bounded consolidation process over recent episodes and durable preferences.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConsolidationEngine;

impl ConsolidationEngine {
    pub fn consolidate<S: MemoryStore>(
        &self,
        store: &mut S,
        episode_memory: Option<&DurableMemory>,
        primary_memory: Option<&DurableMemory>,
        clock: &dyn Clock,
    ) -> Result<Vec<ConsolidationOutcome>> {
        let now = clock.now();
        let window_start = now - Duration::days(CONSOLIDATION_WINDOW_DAYS);
        let mut outcomes = Vec::new();

        if let Some(memory) = primary_memory {
            if let Some(outcome) = self.self_model_outcome(store, memory, window_start, now)? {
                outcomes.push(outcome);
            }
            if let Some(outcome) = self.belief_window_outcome(store, memory, window_start, now)? {
                outcomes.push(outcome);
            }
            if let Some(outcome) = self.blocker_outcome(store, memory, window_start, now)? {
                outcomes.push(outcome);
            }
        }

        if let Some(episode) = episode_memory
            && let Some(workflow_key) = episode.metadata.get("workflow_key")
            && Self::is_success_outcome(episode.metadata.get("outcome").map(String::as_str))
        {
            let successful_episodes = {
                let mut episode_store = EpisodeStore::new(store);
                episode_store
                    .list_by_workflow_key(workflow_key)?
                    .into_iter()
                    .filter(|record| record.event_at >= window_start)
                    .filter(|record| Self::is_success_outcome(record.outcome.as_deref()))
                    .collect::<Vec<_>>()
            };

            if successful_episodes.len() >= 2 {
                let source_memory_ids: Vec<_> = successful_episodes
                    .iter()
                    .map(|record| record.memory_id.clone())
                    .collect();
                let description = episode
                    .metadata
                    .get("procedure_description")
                    .cloned()
                    .unwrap_or_else(|| {
                        format!(
                            "workflow {workflow_key} has succeeded repeatedly and should be reused"
                        )
                    });
                let procedure_outcome = {
                    let mut procedure_store = ProcedureStore::new(store);
                    procedure_store.upsert_success(
                        workflow_key,
                        &description,
                        &source_memory_ids,
                        now,
                    )?
                };
                let record = ConsolidationRecord {
                    consolidation_id: Uuid::new_v4().to_string(),
                    target_layer: MemoryLayer::Procedure,
                    target_id: Some(procedure_outcome.record.procedure_id.clone()),
                    source_memory_ids,
                    reason: format!(
                        "repeated successful workflow {workflow_key} promoted into procedure memory"
                    ),
                    confidence: procedure_outcome.record.confidence,
                    created_at: now,
                    metadata: {
                        let mut metadata = BTreeMap::from([
                            ("workflow_key".to_string(), workflow_key.clone()),
                            (
                                "window_days".to_string(),
                                CONSOLIDATION_WINDOW_DAYS.to_string(),
                            ),
                        ]);
                        if let Some(transition) = &procedure_outcome.status_transition {
                            metadata.insert(
                                "previous_procedure_status".to_string(),
                                transition.previous_status.as_str().to_string(),
                            );
                            metadata.insert(
                                "next_procedure_status".to_string(),
                                transition.next_status.as_str().to_string(),
                            );
                        }
                        metadata
                    },
                };
                let trace_id = self.persist_record(store, &record)?;
                outcomes.push(ConsolidationOutcome {
                    record,
                    trace_id,
                    learned_procedure_id: Some(procedure_outcome.record.procedure_id),
                    procedure_status_transition: procedure_outcome.status_transition,
                });
            }
        }

        Ok(outcomes)
    }

    fn self_model_outcome<S: MemoryStore>(
        &self,
        store: &mut S,
        memory: &DurableMemory,
        window_start: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Option<ConsolidationOutcome>> {
        if memory.memory_layer() != MemoryLayer::SelfModel || memory.is_retraction {
            return Ok(None);
        }

        let matching = {
            let mut self_model_store = SelfModelStore::new(store);
            self_model_store.matching_values_since(
                &memory.entity,
                &memory.slot,
                &memory.value,
                window_start,
            )?
        };
        if matching.len() != 2 {
            return Ok(None);
        }

        let record = ConsolidationRecord {
            consolidation_id: Uuid::new_v4().to_string(),
            target_layer: MemoryLayer::SelfModel,
            target_id: Some(memory.memory_id.clone()),
            source_memory_ids: matching
                .into_iter()
                .map(|record| record.memory_id)
                .collect(),
            reason: "repeated self-model observations stabilized into durable preference"
                .to_string(),
            confidence: memory.confidence,
            created_at: now,
            metadata: BTreeMap::from([
                ("entity".to_string(), memory.entity.clone()),
                ("slot".to_string(), memory.slot.clone()),
                ("value".to_string(), memory.value.clone()),
                (
                    "window_days".to_string(),
                    CONSOLIDATION_WINDOW_DAYS.to_string(),
                ),
            ]),
        };
        let trace_id = self.persist_record(store, &record)?;
        Ok(Some(ConsolidationOutcome {
            record,
            trace_id,
            learned_procedure_id: None,
            procedure_status_transition: None,
        }))
    }

    fn belief_window_outcome<S: MemoryStore>(
        &self,
        store: &mut S,
        memory: &DurableMemory,
        window_start: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Option<ConsolidationOutcome>> {
        if memory.memory_layer() != MemoryLayer::Belief || memory.is_retraction {
            return Ok(None);
        }

        let mut matching: Vec<_> = store
            .list_memories_for_belief(&memory.entity, &memory.slot)?
            .into_iter()
            .filter(|candidate| !candidate.is_retraction)
            .filter(|candidate| candidate.value == memory.value)
            .filter(|candidate| candidate.event_timestamp() >= window_start)
            .collect();
        matching.sort_by(|left, right| left.event_timestamp().cmp(&right.event_timestamp()));
        if matching.len() < 2 {
            return Ok(None);
        }

        let span = matching
            .last()
            .zip(matching.first())
            .map(|(latest, earliest)| latest.event_timestamp() - earliest.event_timestamp())
            .unwrap_or_else(Duration::zero);
        if span < Duration::days(BELIEF_STABILITY_MIN_DAYS) {
            return Ok(None);
        }

        let record = ConsolidationRecord {
            consolidation_id: Uuid::new_v4().to_string(),
            target_layer: MemoryLayer::Belief,
            target_id: Some(memory.memory_id.clone()),
            source_memory_ids: matching
                .into_iter()
                .map(|candidate| candidate.memory_id)
                .collect(),
            reason: "consistent belief evidence remained stable across a bounded window"
                .to_string(),
            confidence: memory.confidence,
            created_at: now,
            metadata: BTreeMap::from([
                ("entity".to_string(), memory.entity.clone()),
                ("slot".to_string(), memory.slot.clone()),
                ("value".to_string(), memory.value.clone()),
                (
                    "window_days".to_string(),
                    CONSOLIDATION_WINDOW_DAYS.to_string(),
                ),
                (
                    "stability_days".to_string(),
                    BELIEF_STABILITY_MIN_DAYS.to_string(),
                ),
            ]),
        };
        let trace_id = self.persist_record(store, &record)?;
        Ok(Some(ConsolidationOutcome {
            record,
            trace_id,
            learned_procedure_id: None,
            procedure_status_transition: None,
        }))
    }

    fn blocker_outcome<S: MemoryStore>(
        &self,
        store: &mut S,
        memory: &DurableMemory,
        window_start: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Option<ConsolidationOutcome>> {
        if memory.memory_layer() != MemoryLayer::GoalState || memory.is_retraction {
            return Ok(None);
        }

        let Some(goal) = memory.to_goal_record() else {
            return Ok(None);
        };
        let Some(blocker_key) = GoalStateStore::<S>::blocker_key(&goal) else {
            return Ok(None);
        };

        let matching = {
            let mut goal_store = GoalStateStore::new(store);
            goal_store
                .list_matching_blockers(&goal.entity, &goal.slot, &blocker_key)?
                .into_iter()
                .filter(|record| record.updated_at >= window_start)
                .collect::<Vec<_>>()
        };
        if matching.len() != BLOCKER_THRESHOLD {
            return Ok(None);
        }

        let record = ConsolidationRecord {
            consolidation_id: Uuid::new_v4().to_string(),
            target_layer: MemoryLayer::GoalState,
            target_id: Some(goal.goal_id.clone()),
            source_memory_ids: matching
                .into_iter()
                .map(|record| record.memory_id)
                .collect(),
            reason: format!("recurring blocker pattern stabilized for {}", goal.slot),
            confidence: memory.confidence,
            created_at: now,
            metadata: BTreeMap::from([
                ("entity".to_string(), goal.entity),
                ("slot".to_string(), goal.slot),
                ("blocker_key".to_string(), blocker_key),
                (
                    "window_days".to_string(),
                    CONSOLIDATION_WINDOW_DAYS.to_string(),
                ),
                ("threshold".to_string(), BLOCKER_THRESHOLD.to_string()),
            ]),
        };
        let trace_id = self.persist_record(store, &record)?;
        Ok(Some(ConsolidationOutcome {
            record,
            trace_id,
            learned_procedure_id: None,
            procedure_status_transition: None,
        }))
    }

    fn persist_record<S: MemoryStore>(
        &self,
        store: &mut S,
        record: &ConsolidationRecord,
    ) -> Result<String> {
        let raw_text = serde_json::to_string(record)?;
        let metadata = BTreeMap::from([
            (
                "consolidation_id".to_string(),
                record.consolidation_id.clone(),
            ),
            (
                "target_layer".to_string(),
                record.target_layer.as_str().to_string(),
            ),
            ("reason".to_string(), record.reason.clone()),
        ]);
        store.put_trace(&raw_text, metadata)
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
}
