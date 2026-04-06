use std::collections::BTreeMap;

use uuid::Uuid;

use super::adapters::memvid_store::MemoryStore;
use super::clock::Clock;
use super::enums::MemoryLayer;
use super::episode_store::EpisodeStore;
use super::errors::Result;
use super::procedure_store::ProcedureStore;
use super::schemas::{ConsolidationRecord, DurableMemory};
use super::self_model_store::SelfModelStore;

/// Consolidation result emitted after repeated bounded patterns are promoted.
#[derive(Debug, Clone)]
pub struct ConsolidationOutcome {
    pub record: ConsolidationRecord,
    pub trace_id: String,
    pub learned_procedure_id: Option<String>,
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
        let mut outcomes = Vec::new();

        if let Some(memory) = primary_memory {
            if memory.memory_layer() == MemoryLayer::SelfModel && !memory.is_retraction {
                let matching = {
                    let mut self_model_store = SelfModelStore::new(store);
                    self_model_store.matching_values(&memory.entity, &memory.slot, &memory.value)?
                };
                if matching.len() >= 2 {
                    let record = ConsolidationRecord {
                        consolidation_id: Uuid::new_v4().to_string(),
                        target_layer: MemoryLayer::SelfModel,
                        target_id: Some(memory.memory_id.clone()),
                        source_memory_ids: matching
                            .into_iter()
                            .map(|record| record.memory_id)
                            .collect(),
                        reason: "repeated self-model observations stabilized into durable preference".to_string(),
                        confidence: memory.confidence,
                        created_at: clock.now(),
                        metadata: BTreeMap::new(),
                    };
                    let trace_id = self.persist_record(store, &record)?;
                    outcomes.push(ConsolidationOutcome {
                        record,
                        trace_id,
                        learned_procedure_id: None,
                    });
                }
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
                let procedure = {
                    let mut procedure_store = ProcedureStore::new(store);
                    procedure_store.upsert_success(
                        workflow_key,
                        &description,
                        &source_memory_ids,
                        clock.now(),
                    )?
                };
                let record = ConsolidationRecord {
                    consolidation_id: Uuid::new_v4().to_string(),
                    target_layer: MemoryLayer::Procedure,
                    target_id: Some(procedure.procedure_id.clone()),
                    source_memory_ids,
                    reason: format!(
                        "repeated successful workflow {workflow_key} promoted into procedure memory"
                    ),
                    confidence: procedure.confidence,
                    created_at: clock.now(),
                    metadata: BTreeMap::from([(
                        "workflow_key".to_string(),
                        workflow_key.clone(),
                    )]),
                };
                let trace_id = self.persist_record(store, &record)?;
                outcomes.push(ConsolidationOutcome {
                    record,
                    trace_id,
                    learned_procedure_id: Some(procedure.procedure_id),
                });
            }
        }

        Ok(outcomes)
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