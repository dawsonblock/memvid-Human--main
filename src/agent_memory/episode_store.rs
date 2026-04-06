use std::collections::HashSet;

use super::adapters::memvid_store::MemoryStore;
use super::clock::Clock;
use super::enums::MemoryLayer;
use super::errors::{AgentMemoryError, Result};
use super::schemas::{CandidateMemory, DurableMemory, EpisodeRecord};

/// Dedicated bounded store for episodic memory.
pub struct EpisodeStore<'a, S: MemoryStore> {
    store: &'a mut S,
}

impl<'a, S: MemoryStore> EpisodeStore<'a, S> {
    pub fn new(store: &'a mut S) -> Self {
        Self { store }
    }

    pub fn record_candidate(
        &mut self,
        candidate: &CandidateMemory,
        clock: &dyn Clock,
    ) -> Result<DurableMemory> {
        let episode = candidate.to_episode_memory(clock.now());
        self.store.put_memory(&episode)?;
        Ok(episode)
    }

    pub fn save_memory(&mut self, memory: &DurableMemory) -> Result<String> {
        if memory.memory_layer() != MemoryLayer::Episode {
            return Err(AgentMemoryError::InvalidCandidate {
                reason: "episode store can only persist episode-layer memory".to_string(),
            });
        }
        self.store.put_memory(memory)
    }

    pub fn list_recent(&mut self, limit: usize) -> Result<Vec<EpisodeRecord>> {
        let episodes: Vec<_> = self
            .list_recent_memories(limit)?
            .into_iter()
            .map(|memory| memory.to_episode_record())
            .collect();
        Ok(episodes)
    }

    pub fn list_recent_memories(&mut self, limit: usize) -> Result<Vec<DurableMemory>> {
        let mut episodes = self.store.list_memories_by_layer(MemoryLayer::Episode)?;
        episodes.sort_by(|left, right| right.event_timestamp().cmp(&left.event_timestamp()));
        episodes.truncate(limit);
        Ok(episodes)
    }

    pub fn list_recent_for_entity(
        &mut self,
        entity: &str,
        limit: usize,
    ) -> Result<Vec<EpisodeRecord>> {
        let mut episodes: Vec<_> = self
            .store
            .list_memories_by_layer(MemoryLayer::Episode)?
            .into_iter()
            .filter(|memory| memory.entity == entity)
            .collect();
        episodes.sort_by(|left, right| right.event_timestamp().cmp(&left.event_timestamp()));
        episodes.truncate(limit);
        Ok(episodes
            .into_iter()
            .map(|memory| memory.to_episode_record())
            .collect())
    }

    pub fn list_by_memory_ids(&mut self, memory_ids: &[String]) -> Result<Vec<EpisodeRecord>> {
        let wanted: HashSet<&str> = memory_ids.iter().map(String::as_str).collect();
        let mut episodes: Vec<_> = self
            .store
            .list_memories_by_layer(MemoryLayer::Episode)?
            .into_iter()
            .filter(|memory| wanted.contains(memory.memory_id.as_str()))
            .map(|memory| memory.to_episode_record())
            .collect();
        episodes.sort_by(|left, right| right.event_at.cmp(&left.event_at));
        Ok(episodes)
    }

    pub fn list_by_workflow_key(&mut self, workflow_key: &str) -> Result<Vec<EpisodeRecord>> {
        let mut episodes: Vec<_> = self
            .store
            .list_memories_by_layer(MemoryLayer::Episode)?
            .into_iter()
            .filter(|memory| {
                memory
                    .metadata
                    .get("workflow_key")
                    .is_some_and(|value| value == workflow_key)
            })
            .map(|memory| memory.to_episode_record())
            .collect();
        episodes.sort_by(|left, right| right.event_at.cmp(&left.event_at));
        Ok(episodes)
    }

    pub fn list_by_workflow_key_memories(
        &mut self,
        workflow_key: &str,
    ) -> Result<Vec<DurableMemory>> {
        let mut episodes: Vec<_> = self
            .store
            .list_memories_by_layer(MemoryLayer::Episode)?
            .into_iter()
            .filter(|memory| {
                memory
                    .metadata
                    .get("workflow_key")
                    .is_some_and(|value| value == workflow_key)
            })
            .collect();
        episodes.sort_by(|left, right| right.event_timestamp().cmp(&left.event_timestamp()));
        Ok(episodes)
    }
}
