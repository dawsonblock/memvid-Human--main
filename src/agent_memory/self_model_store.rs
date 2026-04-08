use std::collections::HashSet;

use chrono::{DateTime, Utc};

use super::adapters::memvid_store::MemoryStore;
use super::enums::{BeliefStatus, MemoryLayer, SelfModelKind, SelfModelStabilityClass};
use super::errors::{AgentMemoryError, Result};
use super::schemas::{DurableMemory, SelfModelRecord};

/// Dedicated bounded store for durable user and agent operating preferences.
pub struct SelfModelStore<'a, S: MemoryStore> {
    store: &'a mut S,
}

impl<'a, S: MemoryStore> SelfModelStore<'a, S> {
    pub fn new(store: &'a mut S) -> Self {
        Self { store }
    }

    pub(crate) fn save_memory(
        &mut self,
        memory: &DurableMemory,
        supporting_episode_id: Option<&str>,
    ) -> Result<String> {
        if memory.memory_layer() != MemoryLayer::SelfModel {
            return Err(AgentMemoryError::InvalidCandidate {
                reason: "self-model store can only persist self-model memory".to_string(),
            });
        }
        if !memory.has_required_structure_for(MemoryLayer::SelfModel) {
            return Err(AgentMemoryError::InvalidCandidate {
                reason: "self-model store requires non-empty entity, slot, and value".to_string(),
            });
        }

        let entity = memory.entity_non_empty().expect("validated entity");
        let slot = memory.slot_non_empty().expect("validated slot");
        let value = memory.value_non_empty().expect("validated value");

        let existing = {
            let mut candidates: Vec<_> = self
                .store
                .list_memories_by_layer(MemoryLayer::SelfModel)?
                .into_iter()
                .filter(|candidate| candidate.has_required_structure_for(MemoryLayer::SelfModel))
                .filter(|candidate| candidate.entity_non_empty() == Some(entity))
                .filter(|candidate| candidate.slot_non_empty() == Some(slot))
                .collect();
            candidates.sort_by(|left, right| {
                Self::status_priority(right)
                    .cmp(&Self::status_priority(left))
                    .then_with(|| right.version_timestamp().cmp(&left.version_timestamp()))
            });
            candidates.into_iter().next()
        };

        let mut self_model_memory = memory.clone();
        self_model_memory.internal_layer = Some(MemoryLayer::SelfModel);
        self_model_memory.entity = entity.to_string();
        self_model_memory.slot = slot.to_string();
        self_model_memory.value = value.to_string();
        let kind = SelfModelKind::from_slot(&self_model_memory.slot);
        self_model_memory
            .metadata
            .insert("self_model_kind".to_string(), kind.as_str().to_string());
        self_model_memory.metadata.insert(
            "self_model_stability_class".to_string(),
            kind.stability_class().as_str().to_string(),
        );
        self_model_memory.metadata.insert(
            "self_model_update_requirement".to_string(),
            kind.update_requirement().as_str().to_string(),
        );
        self_model_memory
            .metadata
            .entry("reinforcement_count".to_string())
            .or_insert_with(|| "1".to_string());
        self_model_memory.metadata.insert(
            "self_model_status".to_string(),
            BeliefStatus::Active.as_str().to_string(),
        );

        if let Some(existing_memory) = existing {
            let mut supporting_ids = Self::supporting_ids(&existing_memory);
            if let Some(episode_id) = supporting_episode_id
                && !supporting_ids
                    .iter()
                    .any(|existing_id| existing_id == episode_id)
            {
                supporting_ids.push(episode_id.to_string());
            }

            if existing_memory.value == self_model_memory.value {
                let reinforcement_count = existing_memory
                    .metadata
                    .get("reinforcement_count")
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(1)
                    + 1;
                self_model_memory.memory_id = existing_memory.memory_id.clone();
                self_model_memory.stored_at = existing_memory.stored_at;
                self_model_memory.confidence =
                    self_model_memory.confidence.max(existing_memory.confidence);
                self_model_memory.metadata.insert(
                    "reinforcement_count".to_string(),
                    reinforcement_count.to_string(),
                );
                self_model_memory.metadata.insert(
                    "last_reinforced_at".to_string(),
                    self_model_memory.version_timestamp().to_rfc3339(),
                );
                self_model_memory.metadata.insert(
                    "supporting_memory_ids".to_string(),
                    supporting_ids.join(","),
                );
                self_model_memory.metadata.insert(
                    "self_model_stability_class".to_string(),
                    kind.stability_class().as_str().to_string(),
                );
            } else {
                let new_strength =
                    self_model_memory.confidence + self_model_memory.source.trust_weight;
                let existing_strength =
                    existing_memory.confidence + existing_memory.source.trust_weight;
                let forced_update = self_model_memory
                    .metadata
                    .contains_key("stable_directive_update_path");
                self_model_memory
                    .metadata
                    .insert("prior_value".to_string(), existing_memory.value.clone());
                self_model_memory.metadata.insert(
                    "conflict_observed_at".to_string(),
                    self_model_memory.version_timestamp().to_rfc3339(),
                );
                self_model_memory.metadata.insert(
                    "supporting_memory_ids".to_string(),
                    supporting_ids.join(","),
                );
                if forced_update || new_strength + 0.05 >= existing_strength {
                    self_model_memory.memory_id = existing_memory.memory_id.clone();
                    self_model_memory.stored_at = existing_memory.stored_at;
                    self_model_memory.metadata.insert(
                        "contradiction_resolution".to_string(),
                        "updated".to_string(),
                    );
                } else {
                    self_model_memory.metadata.insert(
                        "self_model_status".to_string(),
                        BeliefStatus::Disputed.as_str().to_string(),
                    );
                    self_model_memory.metadata.insert(
                        "contradiction_resolution".to_string(),
                        "disputed".to_string(),
                    );
                }
            }
        }

        if let Some(episode_id) = supporting_episode_id {
            self_model_memory = self_model_memory.with_supporting_episode(episode_id);
        }

        self.store.put_memory(&self_model_memory)
    }

    pub fn list_for_entity(&mut self, entity: &str) -> Result<Vec<SelfModelRecord>> {
        let records: Vec<_> = self
            .list_for_entity_memories(entity)?
            .into_iter()
            .filter_map(|memory| memory.to_self_model_record())
            .collect();
        Ok(records)
    }

    pub fn list_for_entity_memories(&mut self, entity: &str) -> Result<Vec<DurableMemory>> {
        let entity = entity.trim();
        if entity.is_empty() {
            return Ok(Vec::new());
        }

        let mut memories: Vec<_> = self
            .store
            .list_memories_by_layer(MemoryLayer::SelfModel)?
            .into_iter()
            .filter(|memory| memory.has_required_structure_for(MemoryLayer::SelfModel))
            .filter(|memory| memory.entity_non_empty() == Some(entity))
            .collect();
        memories.sort_by(|left, right| {
            Self::status_priority(right)
                .cmp(&Self::status_priority(left))
                .then_with(|| right.version_timestamp().cmp(&left.version_timestamp()))
        });
        Ok(memories)
    }

    pub fn get_latest_for_entity_slot(
        &mut self,
        entity: &str,
        slot: &str,
    ) -> Result<Option<SelfModelRecord>> {
        let slot = slot.trim();
        if slot.is_empty() {
            return Ok(None);
        }

        Ok(self
            .list_for_entity(entity)?
            .into_iter()
            .find(|record| record.slot == slot))
    }

    pub fn list_latest_for_entity(&mut self, entity: &str) -> Result<Vec<SelfModelRecord>> {
        let mut seen_slots = HashSet::new();
        let mut latest = Vec::new();
        for record in self.list_for_entity(entity)? {
            if seen_slots.insert(record.slot.clone()) {
                latest.push(record);
            }
        }
        Ok(latest)
    }

    pub fn matching_values(
        &mut self,
        entity: &str,
        slot: &str,
        value: &str,
    ) -> Result<Vec<SelfModelRecord>> {
        let slot = slot.trim();
        let value = value.trim();
        if slot.is_empty() || value.is_empty() {
            return Ok(Vec::new());
        }

        Ok(self
            .store
            .list_memory_versions_by_layer(MemoryLayer::SelfModel)?
            .into_iter()
            .filter(|memory| memory.has_required_structure_for(MemoryLayer::SelfModel))
            .filter(|memory| memory.entity_non_empty() == Some(entity))
            .filter(|memory| memory.slot == slot && memory.value == value)
            .filter_map(|memory| memory.to_self_model_record())
            .filter(|record| record.slot == slot && record.value == value)
            .collect())
    }

    pub fn matching_values_since(
        &mut self,
        entity: &str,
        slot: &str,
        value: &str,
        since: DateTime<Utc>,
    ) -> Result<Vec<SelfModelRecord>> {
        Ok(self
            .matching_values(entity, slot, value)?
            .into_iter()
            .filter(|record| record.observed_at >= since)
            .collect())
    }

    fn supporting_ids(memory: &DurableMemory) -> Vec<String> {
        memory
            .metadata
            .get("supporting_memory_ids")
            .or_else(|| memory.metadata.get("supporting_episode_ids"))
            .map(|value| {
                value
                    .split(',')
                    .filter(|entry| !entry.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn status_priority(memory: &DurableMemory) -> u8 {
        match memory
            .metadata
            .get("self_model_status")
            .and_then(|value| BeliefStatus::from_str(value))
            .unwrap_or(BeliefStatus::Active)
        {
            BeliefStatus::Active => match memory
                .metadata
                .get("self_model_stability_class")
                .and_then(|value| SelfModelStabilityClass::from_str(value))
                .unwrap_or(SelfModelStabilityClass::FlexiblePreference)
            {
                SelfModelStabilityClass::StableDirective => 4,
                SelfModelStabilityClass::FlexiblePreference => 3,
            },
            BeliefStatus::Stale => 2,
            BeliefStatus::Disputed => 1,
            BeliefStatus::Retracted => 0,
        }
    }
}
