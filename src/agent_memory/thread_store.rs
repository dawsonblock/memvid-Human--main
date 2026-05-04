//! Memory threading — groups related memories into named conversation threads
//! and supports parent-child linkage between episodes.

use std::collections::BTreeMap;

use super::clock::Clock;

/// A named thread that groups related memory IDs.
#[derive(Debug, Clone)]
pub struct MemoryThread {
    pub thread_id: String,
    pub entity: String,
    pub created_at: i64,
    pub closed_at: Option<i64>,
    pub memory_ids: Vec<String>,
}

/// Manages open and closed conversation threads.
#[derive(Debug, Default, Clone)]
pub struct MemoryThreadStore {
    threads: BTreeMap<String, MemoryThread>,
}

impl MemoryThreadStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a new thread for `entity`. Returns the newly created
    /// [`MemoryThread`]; the thread is stored internally and also returned
    /// so callers can capture the `thread_id` immediately.
    pub fn open_thread(&mut self, entity: &str, clock: &dyn Clock) -> MemoryThread {
        let thread_id = uuid::Uuid::new_v4().to_string();
        let thread = MemoryThread {
            thread_id: thread_id.clone(),
            entity: entity.to_string(),
            created_at: clock.now().timestamp(),
            closed_at: None,
            memory_ids: Vec::new(),
        };
        self.threads.insert(thread_id, thread.clone());
        thread
    }

    /// Attach a memory ID to an existing thread.  Returns `true` when the
    /// memory was added, `false` when the thread does not exist or is already
    /// closed.
    pub fn attach_memory(&mut self, thread_id: &str, memory_id: &str) -> bool {
        match self.threads.get_mut(thread_id) {
            Some(thread) if thread.closed_at.is_none() => {
                if !thread.memory_ids.contains(&memory_id.to_string()) {
                    thread.memory_ids.push(memory_id.to_string());
                }
                true
            }
            _ => false,
        }
    }

    /// Return the ordered slice of memory IDs attached to `thread_id`.
    #[must_use]
    pub fn get_thread_memories(&self, thread_id: &str) -> Vec<&str> {
        self.threads
            .get(thread_id)
            .map(|t| t.memory_ids.iter().map(|id| id.as_str()).collect())
            .unwrap_or_default()
    }

    /// Close a thread so no further memories can be attached.  Returns `true`
    /// when the thread was open and is now closed, `false` if it did not exist
    /// or was already closed.
    pub fn close_thread(&mut self, thread_id: &str, clock: &dyn Clock) -> bool {
        match self.threads.get_mut(thread_id) {
            Some(thread) if thread.closed_at.is_none() => {
                thread.closed_at = Some(clock.now().timestamp());
                true
            }
            _ => false,
        }
    }

    /// Return all open threads for `entity`.
    #[must_use]
    pub fn active_threads_for(&self, entity: &str) -> Vec<&MemoryThread> {
        self.threads
            .values()
            .filter(|t| t.entity == entity && t.closed_at.is_none())
            .collect()
    }

    /// Return a reference to a thread by ID, regardless of status.
    #[must_use]
    pub fn get_thread(&self, thread_id: &str) -> Option<&MemoryThread> {
        self.threads.get(thread_id)
    }

    /// Number of threads (open and closed) currently held.
    #[must_use]
    pub fn thread_count(&self) -> usize {
        self.threads.len()
    }
}
