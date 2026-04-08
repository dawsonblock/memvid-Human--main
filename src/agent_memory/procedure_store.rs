use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::adapters::memvid_store::MemoryStore;
use super::enums::{
    MemoryLayer, MemoryType, OutcomeFeedbackKind, ProcedureStatus, Scope, SourceType,
};
use super::errors::{AgentMemoryError, Result};
use super::schemas::{DurableMemory, ProcedureRecord, Provenance};

const POSITIVE_OUTCOME_COUNT_KEY: &str = "positive_outcome_count";
const NEGATIVE_OUTCOME_COUNT_KEY: &str = "negative_outcome_count";
const LAST_OUTCOME_AT_KEY: &str = "last_outcome_at";
const LAST_POSITIVE_OUTCOME_AT_KEY: &str = "last_positive_outcome_at";
const LAST_NEGATIVE_OUTCOME_AT_KEY: &str = "last_negative_outcome_at";
const OUTCOME_IMPACT_SCORE_KEY: &str = "outcome_impact_score";
const LAST_FEEDBACK_OUTCOME_KEY: &str = "last_feedback_outcome";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcedureStatusTransition {
    pub procedure_id: String,
    pub workflow_key: String,
    pub previous_status: ProcedureStatus,
    pub next_status: ProcedureStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcedureUpsertOutcome {
    pub record: ProcedureRecord,
    pub status_transition: Option<ProcedureStatusTransition>,
}

#[must_use]
pub fn effective_procedure_status(record: &ProcedureRecord) -> ProcedureStatus {
    if record.status == ProcedureStatus::Retired {
        return ProcedureStatus::Retired;
    }
    if record.status == ProcedureStatus::CoolingDown {
        return ProcedureStatus::CoolingDown;
    }

    let total_runs = record.success_count + record.failure_count;
    if total_runs >= 5 && record.failure_count >= record.success_count.saturating_add(3) {
        ProcedureStatus::Retired
    } else if total_runs >= 3 && record.failure_count > record.success_count {
        ProcedureStatus::CoolingDown
    } else {
        ProcedureStatus::Active
    }
}

fn workflow_key_for(record: &ProcedureRecord) -> String {
    record
        .metadata
        .get("workflow_key")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| record.name.trim())
        .to_string()
}

fn procedure_confidence(success_count: u32, failure_count: u32) -> f32 {
    let total = success_count + failure_count;
    if total == 0 {
        return 0.55;
    }

    let success_ratio = success_count as f32 / total as f32;
    (0.3 + (success_ratio * 0.68)).clamp(0.12, 0.98)
}

fn metadata_outcome_count(metadata: &BTreeMap<String, String>, key: &str) -> u32 {
    metadata
        .get(key)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0)
}

fn metadata_outcome_impact_score(metadata: &BTreeMap<String, String>) -> f32 {
    let positive = metadata_outcome_count(metadata, POSITIVE_OUTCOME_COUNT_KEY);
    let negative = metadata_outcome_count(metadata, NEGATIVE_OUTCOME_COUNT_KEY);
    let total = positive + negative;
    if total == 0 {
        0.0
    } else {
        ((positive as f32 - negative as f32) / total as f32).clamp(-1.0, 1.0)
    }
}

fn apply_outcome_feedback_metadata(
    metadata: &mut BTreeMap<String, String>,
    outcome: OutcomeFeedbackKind,
    observed_at: DateTime<Utc>,
) {
    let positive = metadata_outcome_count(metadata, POSITIVE_OUTCOME_COUNT_KEY);
    let negative = metadata_outcome_count(metadata, NEGATIVE_OUTCOME_COUNT_KEY);
    match outcome {
        OutcomeFeedbackKind::Positive => {
            metadata.insert(
                POSITIVE_OUTCOME_COUNT_KEY.to_string(),
                positive.saturating_add(1).to_string(),
            );
            metadata.insert(
                LAST_POSITIVE_OUTCOME_AT_KEY.to_string(),
                observed_at.to_rfc3339(),
            );
        }
        OutcomeFeedbackKind::Negative => {
            metadata.insert(
                NEGATIVE_OUTCOME_COUNT_KEY.to_string(),
                negative.saturating_add(1).to_string(),
            );
            metadata.insert(
                LAST_NEGATIVE_OUTCOME_AT_KEY.to_string(),
                observed_at.to_rfc3339(),
            );
        }
    }
    metadata.insert(LAST_OUTCOME_AT_KEY.to_string(), observed_at.to_rfc3339());
    metadata.insert(
        LAST_FEEDBACK_OUTCOME_KEY.to_string(),
        outcome.as_str().to_string(),
    );
    metadata.insert(
        OUTCOME_IMPACT_SCORE_KEY.to_string(),
        format!("{:.6}", metadata_outcome_impact_score(metadata)),
    );
}

/// Dedicated bounded store for learned operational procedures.
pub struct ProcedureStore<'a, S: MemoryStore> {
    store: &'a mut S,
}

impl<'a, S: MemoryStore> ProcedureStore<'a, S> {
    pub fn new(store: &'a mut S) -> Self {
        Self { store }
    }

    pub(crate) fn save_memory(&mut self, memory: &DurableMemory) -> Result<String> {
        if memory.memory_layer() != MemoryLayer::Procedure {
            return Err(AgentMemoryError::InvalidCandidate {
                reason: "procedure store can only persist procedure-layer memory".to_string(),
            });
        }
        if !memory.has_required_structure_for(MemoryLayer::Procedure) {
            return Err(AgentMemoryError::InvalidCandidate {
                reason: "procedure store requires non-empty entity, slot, value, and workflow key"
                    .to_string(),
            });
        }

        let workflow_key = memory
            .workflow_key_non_empty()
            .expect("validated workflow key")
            .to_string();
        let mut procedure_memory = memory.clone();
        procedure_memory.internal_layer = Some(MemoryLayer::Procedure);
        procedure_memory.entity = memory
            .entity_non_empty()
            .expect("validated entity")
            .to_string();
        procedure_memory.slot = memory.slot_non_empty().expect("validated slot").to_string();
        procedure_memory.value = memory
            .value_non_empty()
            .expect("validated value")
            .to_string();
        procedure_memory
            .metadata
            .insert("workflow_key".to_string(), workflow_key.clone());
        let procedure_name = procedure_memory
            .metadata
            .get("procedure_name")
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| procedure_memory.value.as_str())
            .to_string();
        procedure_memory
            .metadata
            .insert("procedure_name".to_string(), procedure_name);
        if let Some(existing) = self.get_by_workflow_key(&workflow_key)? {
            procedure_memory.memory_id = existing.procedure_id;
            let recorded_at = existing
                .metadata
                .get("recorded_at")
                .and_then(|recorded_at| DateTime::parse_from_rfc3339(recorded_at).ok())
                .map(|recorded_at| recorded_at.with_timezone(&Utc))
                .unwrap_or(existing.updated_at);
            procedure_memory.stored_at = recorded_at;
            procedure_memory
                .metadata
                .insert("recorded_at".to_string(), recorded_at.to_rfc3339());
        }
        self.store.put_memory(&procedure_memory)
    }

    pub fn list_all(&mut self) -> Result<Vec<ProcedureRecord>> {
        let records: Vec<_> = self
            .list_all_memories()?
            .into_iter()
            .filter_map(|memory| memory.to_procedure_record())
            .collect();
        Ok(records)
    }

    pub fn list_all_memories(&mut self) -> Result<Vec<DurableMemory>> {
        let mut memories = self.store.list_memories_by_layer(MemoryLayer::Procedure)?;
        memories.sort_by(|left, right| right.version_timestamp().cmp(&left.version_timestamp()));
        let mut seen_workflows = HashSet::new();
        let mut latest = Vec::new();
        for memory in memories {
            let Some(record) = memory.to_procedure_record() else {
                continue;
            };
            let workflow_key = workflow_key_for(&record);
            if seen_workflows.insert(workflow_key) {
                latest.push(memory);
            }
        }
        Ok(latest)
    }

    pub fn list_active(&mut self) -> Result<Vec<ProcedureRecord>> {
        Ok(self
            .list_all()?
            .into_iter()
            .filter(|record| effective_procedure_status(record) != ProcedureStatus::Retired)
            .collect())
    }

    pub fn list_by_context_tag(&mut self, tag: &str) -> Result<Vec<ProcedureRecord>> {
        let tag_lower = tag.to_lowercase();
        Ok(self
            .list_active()?
            .into_iter()
            .filter(|record| {
                record
                    .context_tags
                    .iter()
                    .any(|existing| existing.to_lowercase() == tag_lower)
            })
            .collect())
    }

    pub fn get_by_workflow_key(&mut self, workflow_key: &str) -> Result<Option<ProcedureRecord>> {
        let workflow_key = workflow_key.trim();
        if workflow_key.is_empty() {
            return Ok(None);
        }

        Ok(self
            .list_all()?
            .into_iter()
            .find(|record| workflow_key_for(record) == workflow_key))
    }

    pub(crate) fn upsert_success(
        &mut self,
        workflow_key: &str,
        description: &str,
        learned_from_memory_ids: &[String],
        now: DateTime<Utc>,
    ) -> Result<ProcedureUpsertOutcome> {
        let workflow_key = workflow_key.trim();
        if workflow_key.is_empty() {
            return Err(AgentMemoryError::InvalidCandidate {
                reason: "procedure workflow key cannot be blank".to_string(),
            });
        }

        let existing = self.get_by_workflow_key(workflow_key)?;
        let success_count = existing
            .as_ref()
            .map_or(learned_from_memory_ids.len() as u32, |record| {
                record.success_count + 1
            });
        let failure_count = existing.as_ref().map_or(0, |record| record.failure_count);
        let record = self.build_record(
            existing.as_ref(),
            workflow_key,
            description,
            learned_from_memory_ids,
            success_count,
            failure_count,
            now,
            Some(now),
            None,
        );
        self.persist_record_outcome(existing.as_ref(), workflow_key, record, now)
    }

    pub(crate) fn record_failure(
        &mut self,
        workflow_key: &str,
        learned_from_memory_ids: &[String],
        now: DateTime<Utc>,
    ) -> Result<Option<ProcedureUpsertOutcome>> {
        let workflow_key = workflow_key.trim();
        if workflow_key.is_empty() {
            return Ok(None);
        }

        let existing = self.get_by_workflow_key(workflow_key)?;
        let Some(existing_record) = existing.as_ref() else {
            return Ok(None);
        };

        let record = self.build_record(
            Some(existing_record),
            workflow_key,
            &existing_record.description,
            learned_from_memory_ids,
            existing_record.success_count,
            existing_record.failure_count + 1,
            now,
            None,
            Some(now),
        );
        self.persist_record_outcome(Some(existing_record), workflow_key, record, now)
            .map(Some)
    }

    pub(crate) fn record_feedback(
        &mut self,
        workflow_key: &str,
        outcome: OutcomeFeedbackKind,
        now: DateTime<Utc>,
    ) -> Result<Option<ProcedureUpsertOutcome>> {
        let workflow_key = workflow_key.trim();
        if workflow_key.is_empty() {
            return Ok(None);
        }

        let existing = self.get_by_workflow_key(workflow_key)?;
        let Some(existing_record) = existing.as_ref() else {
            return Ok(None);
        };

        let mut record = match outcome {
            OutcomeFeedbackKind::Positive => self.build_record(
                Some(existing_record),
                workflow_key,
                &existing_record.description,
                &[],
                existing_record.success_count + 1,
                existing_record.failure_count,
                now,
                Some(now),
                None,
            ),
            OutcomeFeedbackKind::Negative => self.build_record(
                Some(existing_record),
                workflow_key,
                &existing_record.description,
                &[],
                existing_record.success_count,
                existing_record.failure_count + 1,
                now,
                None,
                Some(now),
            ),
        };
        apply_outcome_feedback_metadata(&mut record.metadata, outcome, now);
        self.persist_record_outcome(Some(existing_record), workflow_key, record, now)
            .map(Some)
    }

    pub(crate) fn sync_all_effective_statuses(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<Vec<ProcedureStatusTransition>> {
        let mut seen_workflows = HashSet::new();
        let mut transitions = Vec::new();

        for record in self.list_all()? {
            let workflow_key = workflow_key_for(&record);
            if !seen_workflows.insert(workflow_key.clone()) {
                continue;
            }

            let effective_status = effective_procedure_status(&record);
            if effective_status == record.status {
                continue;
            }

            let mut updated = record.clone();
            updated.status = effective_status;
            updated.updated_at = now;
            updated.metadata.insert(
                "prior_procedure_status".to_string(),
                record.status.as_str().to_string(),
            );
            updated
                .metadata
                .insert("status_transition_at".to_string(), now.to_rfc3339());

            self.persist_record(&updated)?;
            transitions.push(ProcedureStatusTransition {
                procedure_id: updated.procedure_id,
                workflow_key,
                previous_status: record.status,
                next_status: effective_status,
            });
        }

        Ok(transitions)
    }

    fn build_record(
        &self,
        existing: Option<&ProcedureRecord>,
        workflow_key: &str,
        description: &str,
        learned_from_memory_ids: &[String],
        success_count: u32,
        failure_count: u32,
        now: DateTime<Utc>,
        last_succeeded_at: Option<DateTime<Utc>>,
        last_failed_at: Option<DateTime<Utc>>,
    ) -> ProcedureRecord {
        let mut merged_memory_ids = existing
            .map(|record| record.learned_from_memory_ids.clone())
            .unwrap_or_default();
        for memory_id in learned_from_memory_ids {
            if !merged_memory_ids
                .iter()
                .any(|existing_id| existing_id == memory_id)
            {
                merged_memory_ids.push(memory_id.clone());
            }
        }

        let mut metadata = existing
            .map(|record| record.metadata.clone())
            .unwrap_or_default();
        metadata.insert("workflow_key".to_string(), workflow_key.to_string());
        metadata.insert("procedure_name".to_string(), workflow_key.to_string());
        if let Some(last_failed_at) = last_failed_at {
            metadata.insert("last_failed_at".to_string(), last_failed_at.to_rfc3339());
        }

        let mut record = ProcedureRecord {
            procedure_id: existing
                .map(|record| record.procedure_id.clone())
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            name: workflow_key.to_string(),
            description: description.to_string(),
            context_tags: existing
                .map(|record| record.context_tags.clone())
                .unwrap_or_else(|| vec![workflow_key.to_string()]),
            success_count,
            failure_count,
            confidence: procedure_confidence(success_count, failure_count),
            status: ProcedureStatus::Active,
            learned_from_memory_ids: merged_memory_ids,
            last_used_at: Some(now),
            last_succeeded_at: last_succeeded_at
                .or_else(|| existing.and_then(|record| record.last_succeeded_at)),
            last_failed_at: last_failed_at
                .or_else(|| existing.and_then(|record| record.last_failed_at)),
            updated_at: now,
            metadata,
        };
        record.status = effective_procedure_status(&record);
        record
    }

    fn persist_record_outcome(
        &mut self,
        existing: Option<&ProcedureRecord>,
        workflow_key: &str,
        mut record: ProcedureRecord,
        now: DateTime<Utc>,
    ) -> Result<ProcedureUpsertOutcome> {
        let previous_status = existing.map(|record| record.status);
        let status_transition = previous_status.and_then(|previous_status| {
            (previous_status != record.status).then(|| ProcedureStatusTransition {
                procedure_id: record.procedure_id.clone(),
                workflow_key: workflow_key.to_string(),
                previous_status,
                next_status: record.status,
            })
        });
        if let Some(transition) = &status_transition {
            record.metadata.insert(
                "prior_procedure_status".to_string(),
                transition.previous_status.as_str().to_string(),
            );
            record
                .metadata
                .insert("status_transition_at".to_string(), now.to_rfc3339());
        }

        self.persist_record(&record)?;
        Ok(ProcedureUpsertOutcome {
            record,
            status_transition,
        })
    }

    fn persist_record(&mut self, record: &ProcedureRecord) -> Result<String> {
        let mut metadata: BTreeMap<String, String> = record.metadata.clone();
        metadata.insert("procedure_name".to_string(), record.name.clone());
        metadata.insert("context_tags".to_string(), record.context_tags.join(","));
        metadata.insert(
            "success_count".to_string(),
            record.success_count.to_string(),
        );
        metadata.insert(
            "failure_count".to_string(),
            record.failure_count.to_string(),
        );
        metadata.insert(
            "procedure_status".to_string(),
            record.status.as_str().to_string(),
        );
        metadata.insert(
            "learned_from_memory_ids".to_string(),
            record.learned_from_memory_ids.join(","),
        );
        if let Some(last_used_at) = record.last_used_at {
            metadata.insert("last_used_at".to_string(), last_used_at.to_rfc3339());
        }
        if let Some(last_succeeded_at) = record.last_succeeded_at {
            metadata.insert(
                "last_succeeded_at".to_string(),
                last_succeeded_at.to_rfc3339(),
            );
        }
        if let Some(last_failed_at) = record.last_failed_at {
            metadata.insert("last_failed_at".to_string(), last_failed_at.to_rfc3339());
        }
        let recorded_at = metadata
            .get("recorded_at")
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
            .unwrap_or(record.updated_at);
        metadata.insert("recorded_at".to_string(), recorded_at.to_rfc3339());

        let memory = DurableMemory {
            memory_id: record.procedure_id.clone(),
            candidate_id: format!("procedure-{}", record.procedure_id),
            stored_at: recorded_at,
            updated_at: Some(record.updated_at),
            entity: "procedure".to_string(),
            slot: record
                .metadata
                .get("workflow_key")
                .cloned()
                .unwrap_or_else(|| record.name.clone()),
            value: record.name.clone(),
            raw_text: record.description.clone(),
            memory_type: MemoryType::Trace,
            confidence: record.confidence,
            salience: 0.72,
            scope: Scope::Project,
            ttl: None,
            source: Provenance {
                source_type: SourceType::System,
                source_id: "procedure_store".to_string(),
                source_label: Some("procedure_store".to_string()),
                observed_by: None,
                trust_weight: 1.0,
            },
            event_at: Some(record.updated_at),
            valid_from: Some(record.updated_at),
            valid_to: None,
            internal_layer: Some(MemoryLayer::Procedure),
            tags: record.context_tags.clone(),
            metadata,
            is_retraction: false,
        };

        self.store.put_memory(&memory)
    }
}
