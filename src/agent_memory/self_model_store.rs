use std::collections::HashSet;

use chrono::{DateTime, Utc};

use super::adapters::memvid_store::MemoryStore;
use super::enums::{MemoryLayer, SelfModelKind};
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

    pub fn save_memory(
        &mut self,
        memory: &DurableMemory,
        supporting_episode_id: Option<&str>,
    ) -> Result<String> {
        if memory.memory_layer() != MemoryLayer::SelfModel {
            return Err(AgentMemoryError::InvalidCandidate {
                reason: "self-model store can only persist self-model memory".to_string(),
            });
        }

        let mut self_model_memory = memory.clone();
        self_model_memory.internal_layer = Some(MemoryLayer::SelfModel);
        self_model_memory.metadata.insert(
            "self_model_kind".to_string(),
            SelfModelKind::from_slot(&self_model_memory.slot)
                .as_str()
                .to_string(),
        );
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
        let mut memories: Vec<_> = self
            .store
            .list_memories_by_layer(MemoryLayer::SelfModel)?
            .into_iter()
            .filter(|memory| memory.entity == entity)
            .collect();
        memories.sort_by(|left, right| right.stored_at.cmp(&left.stored_at));
        Ok(memories)
    }

    pub fn get_latest_for_entity_slot(
        &mut self,
        entity: &str,
        slot: &str,
    ) -> Result<Option<SelfModelRecord>> {
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
        Ok(self
            .list_for_entity(entity)?
            .into_iter()
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
}
