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

        let mut goal_memory = memory.clone();
        goal_memory.internal_layer = Some(MemoryLayer::GoalState);
        goal_memory.metadata.insert(
            "goal_status".to_string(),
            GoalStatus::from_text(&goal_memory.value, &goal_memory.raw_text)
                .as_str()
                .to_string(),
        );
        if let Some(episode_id) = supporting_episode_id {
            goal_memory = goal_memory.with_supporting_episode(episode_id);
        }

        self.store.put_memory(&goal_memory)
    }

    pub fn list_all(&mut self) -> Result<Vec<GoalRecord>> {
        let mut records: Vec<_> = self
            .store
            .list_memories_by_layer(MemoryLayer::GoalState)?
            .into_iter()
            .filter_map(|memory| memory.to_goal_record())
            .collect();
        records.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        Ok(records)
    }

    pub fn list_active(&mut self) -> Result<Vec<GoalRecord>> {
        Ok(self
            .list_all()?
            .into_iter()
            .filter(|record| {
                matches!(
                    record.status,
                    GoalStatus::Active
                        | GoalStatus::Blocked
                        | GoalStatus::WaitingOnUser
                        | GoalStatus::WaitingOnSystem
                )
            })
            .collect())
    }
}
