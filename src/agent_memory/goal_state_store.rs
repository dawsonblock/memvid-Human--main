use super::adapters::memvid_store::MemoryStore;
use super::enums::{GoalStatus, MemoryLayer};
use super::errors::{AgentMemoryError, Result};
use super::schemas::{DurableMemory, GoalRecord};

/// Dedicated bounded store for active goal-state memory.
pub struct GoalStateStore<'a, S: MemoryStore> {
    store: &'a mut S,
}

impl<'a, S: MemoryStore> GoalStateStore<'a, S> {
    pub fn new(store: &'a mut S) -> Self {
        Self { store }
    }

    pub fn save_memory(
        &mut self,
        memory: &DurableMemory,
        supporting_episode_id: Option<&str>,
    ) -> Result<String> {
        if memory.memory_layer() != MemoryLayer::GoalState {
            return Err(AgentMemoryError::InvalidCandidate {
                reason: "goal-state store can only persist goal-state memory".to_string(),
            });
        }

        let existing = self
            .store
            .list_memories_by_layer(MemoryLayer::GoalState)?
            .into_iter()
            .filter(|candidate| candidate.entity == memory.entity)
            .filter(|candidate| candidate.slot == memory.slot)
            .max_by(|left, right| left.stored_at.cmp(&right.stored_at));

        let mut goal_memory = memory.clone();
        goal_memory.internal_layer = Some(MemoryLayer::GoalState);
        let goal_status = goal_memory
            .metadata
            .get("goal_status")
            .and_then(|value| GoalStatus::from_str(value))
            .unwrap_or_else(|| GoalStatus::from_text(&goal_memory.value, &goal_memory.raw_text));
        goal_memory
            .metadata
            .insert("goal_status".to_string(), goal_status.as_str().to_string());
        if let Some(existing_memory) = existing {
            goal_memory.memory_id = existing_memory.memory_id;
        }
        if let Some(episode_id) = supporting_episode_id {
            goal_memory = goal_memory.with_supporting_episode(episode_id);
        }

        self.store.put_memory(&goal_memory)
    }

    pub fn list_all(&mut self) -> Result<Vec<GoalRecord>> {
        let records: Vec<_> = self
            .list_all_memories()?
            .into_iter()
            .filter_map(|memory| memory.to_goal_record())
            .collect();
        Ok(records)
    }

    pub fn list_all_memories(&mut self) -> Result<Vec<DurableMemory>> {
        let mut memories = self.store.list_memories_by_layer(MemoryLayer::GoalState)?;
        memories.sort_by(|left, right| right.stored_at.cmp(&left.stored_at));
        Ok(memories)
    }

    pub fn list_active(&mut self) -> Result<Vec<GoalRecord>> {
        Ok(self
            .list_active_memories()?
            .into_iter()
            .filter_map(|memory| memory.to_goal_record())
            .collect())
    }

    pub fn list_active_memories(&mut self) -> Result<Vec<DurableMemory>> {
        let mut latest_keys = std::collections::HashSet::new();
        Ok(self
            .list_all_memories()?
            .into_iter()
            .filter(|memory| {
                latest_keys.insert(format!("{}::{}", memory.entity, memory.slot))
            })
            .filter(|memory| {
                memory
                    .to_goal_record()
                    .is_some_and(|record| Self::is_active_status(record.status))
            })
            .collect())
    }

    pub fn list_for_entity(&mut self, entity: &str) -> Result<Vec<GoalRecord>> {
        Ok(self
            .list_all()?
            .into_iter()
            .filter(|record| record.entity == entity)
            .collect())
    }

    pub fn list_active_for_entity(&mut self, entity: &str) -> Result<Vec<GoalRecord>> {
        Ok(self
            .list_active()?
            .into_iter()
            .filter(|record| record.entity == entity)
            .collect())
    }

    pub fn list_matching_blockers(
        &mut self,
        entity: &str,
        slot: &str,
        blocker_key: &str,
    ) -> Result<Vec<GoalRecord>> {
        Ok(self
            .list_for_entity(entity)?
            .into_iter()
            .filter(|record| record.slot == slot)
            .filter(|record| {
                Self::blocker_key(record).is_some_and(|existing| existing == blocker_key)
            })
            .collect())
    }

    #[must_use]
    pub fn blocker_key(record: &GoalRecord) -> Option<String> {
        match record.status {
            GoalStatus::Blocked | GoalStatus::WaitingOnUser | GoalStatus::WaitingOnSystem => {
                if let Some(reason) = record.metadata.get("blocker_reason") {
                    return Some(reason.trim().to_lowercase());
                }

                let fallback = if record.value.eq_ignore_ascii_case(record.status.as_str()) {
                    record.summary.trim()
                } else {
                    record.value.trim()
                };
                if fallback.is_empty() {
                    None
                } else {
                    Some(fallback.to_lowercase())
                }
            }
            _ => None,
        }
    }

    fn is_active_status(status: GoalStatus) -> bool {
        matches!(
            status,
            GoalStatus::Active
                | GoalStatus::Blocked
                | GoalStatus::WaitingOnUser
                | GoalStatus::WaitingOnSystem
        )
    }
}
