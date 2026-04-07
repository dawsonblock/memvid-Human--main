use super::adapters::memvid_store::MemoryStore;
use super::errors::Result;
use super::schemas::{BeliefRecord, DurableMemory};

/// Belief persistence wrapper over the storage adapter.
pub struct BeliefStore<'a, S: MemoryStore> {
    store: &'a mut S,
}

impl<'a, S: MemoryStore> BeliefStore<'a, S> {
    pub fn new(store: &'a mut S) -> Self {
        Self { store }
    }

    pub fn get(&mut self, entity: &str, slot: &str) -> Result<Option<BeliefRecord>> {
        self.get_current(entity, slot)
    }

    pub fn get_current(&mut self, entity: &str, slot: &str) -> Result<Option<BeliefRecord>> {
        self.store.get_current_belief(entity, slot)
    }

    pub fn get_by_id(&mut self, belief_id: &str) -> Result<Option<BeliefRecord>> {
        self.store.get_belief_by_id(belief_id)
    }

    pub fn get_active(&mut self, entity: &str, slot: &str) -> Result<Option<BeliefRecord>> {
        self.store.get_active_belief(entity, slot)
    }

    pub(crate) fn save(&mut self, belief: &BeliefRecord) -> Result<()> {
        self.store.update_belief(belief)
    }

    pub fn supporting_memories(&mut self, entity: &str, slot: &str) -> Result<Vec<DurableMemory>> {
        self.store.list_memories_for_belief(entity, slot)
    }
}
