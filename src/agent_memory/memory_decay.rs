use chrono::{DateTime, Utc};

use super::adapters::memvid_store::MemoryStore;
use super::errors::Result;
use super::policy::PolicySet;
use super::retention::RetentionManager;
use super::schemas::DurableMemory;

/// Applies retention rules to durable memories when a caller explicitly runs maintenance.
#[derive(Debug, Clone)]
pub struct MemoryDecay {
    retention: RetentionManager,
}

impl MemoryDecay {
    #[must_use]
    pub fn new(retention: RetentionManager) -> Self {
        Self { retention }
    }

    #[must_use]
    pub fn from_policy(policy: PolicySet) -> Self {
        Self::new(RetentionManager::new(policy))
    }

    pub fn run<S: MemoryStore>(
        &self,
        store: &mut S,
        memories: &[DurableMemory],
        now: DateTime<Utc>,
    ) -> Result<Vec<String>> {
        let mut expired_ids = Vec::new();
        for memory in memories {
            if self.retention.evaluate(memory, now).expired {
                store.expire_memory(&memory.memory_id)?;
                expired_ids.push(memory.memory_id.clone());
            }
        }
        Ok(expired_ids)
    }
}
