use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::adapters::memvid_store::MemoryStore;
use super::enums::{MemoryLayer, MemoryType, ProcedureStatus, Scope, SourceType};
use super::errors::{AgentMemoryError, Result};
use super::schemas::{DurableMemory, ProcedureRecord, Provenance};

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
        .cloned()
        .unwrap_or_else(|| record.name.clone())
}

fn procedure_confidence(success_count: u32, failure_count: u32) -> f32 {
    let total = success_count + failure_count;
    if total == 0 {
        return 0.55;
    }

    let success_ratio = success_count as f32 / total as f32;
    (0.3 + (success_ratio * 0.68)).clamp(0.12, 0.98)
}

/// Dedicated bounded store for learned operational procedures.
pub struct ProcedureStore<'a, S: MemoryStore> {
    store: &'a mut S,
}

impl<'a, S: MemoryStore> ProcedureStore<'a, S> {
    pub fn new(store: &'a mut S) -> Self {
        Self { store }
    }

    pub fn save_memory(&mut self, memory: &DurableMemory) -> Result<String> {
        if memory.memory_layer() != MemoryLayer::Procedure {
            return Err(AgentMemoryError::InvalidCandidate {
                reason: "procedure store can only persist procedure-layer memory".to_string(),
            });
        }

        let workflow_key = memory
            .metadata
            .get("workflow_key")
            .cloned()
            .unwrap_or_else(|| memory.slot.clone());
        let mut procedure_memory = memory.clone();
        if let Some(existing) = self.get_by_workflow_key(&workflow_key)? {
            procedure_memory.memory_id = existing.procedure_id;
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
        memories.sort_by(|left, right| right.stored_at.cmp(&left.stored_at));
        let mut seen_workflows = HashSet::new();
        Ok(memories
            .into_iter()
            .filter(|memory| {
                let workflow_key = memory
                    .metadata
                    .get("workflow_key")
                    .cloned()
                    .unwrap_or_else(|| memory.slot.clone());
                seen_workflows.insert(workflow_key)
            })
            .collect())
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
        Ok(self.list_all()?.into_iter().find(|record| {
            record
                .metadata
                .get("workflow_key")
                .is_some_and(|value| value == workflow_key)
        }))
    }

    pub fn upsert_success(
        &mut self,
        workflow_key: &str,
        description: &str,
        learned_from_memory_ids: &[String],
        now: DateTime<Utc>,
    ) -> Result<ProcedureUpsertOutcome> {
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

    pub fn record_failure(
        &mut self,
        workflow_key: &str,
        learned_from_memory_ids: &[String],
        now: DateTime<Utc>,
    ) -> Result<Option<ProcedureUpsertOutcome>> {
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

    pub fn sync_all_effective_statuses(
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

        let memory = DurableMemory {
            memory_id: record.procedure_id.clone(),
            candidate_id: format!("procedure-{}", record.procedure_id),
            stored_at: record.updated_at,
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
