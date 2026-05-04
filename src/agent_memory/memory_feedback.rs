//! Per-memory usefulness feedback store.
//!
//! [`MemoryFeedbackStore`] records explicit feedback signals (helpful, wrong,
//! irrelevant, etc.) against individual memory IDs for the current session.
//! The store is session-scoped and in-memory — it does not persist across
//! restarts.  The [`MemoryController`] integrates it into the retrieval path:
//! suppressed memories are excluded from hits, promoted memories receive a
//! score boost.
//!
//! [`MemoryController`]: crate::agent_memory::memory_controller::MemoryController

use chrono::{DateTime, Utc};

/// Explicit feedback signal about a single retrieved memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedbackSignal {
    /// The memory was directly helpful to the current task.
    Helpful,
    /// The memory contained incorrect information.
    Wrong,
    /// The memory was retrieved but was not useful for the query.
    Irrelevant,
    /// Using this memory caused a bad output (e.g. misleading answer).
    CausedBadOutput,
    /// Explicitly elevate this memory — apply a score boost on future retrieval.
    Promote,
    /// Explicitly suppress this memory — exclude from future retrieval results.
    Suppress,
}

/// A single recorded feedback event for a memory.
#[derive(Debug, Clone)]
pub struct MemoryFeedbackRecord {
    /// ID of the [`DurableMemory`] the feedback refers to.
    pub memory_id: String,
    /// The feedback signal.
    pub signal: FeedbackSignal,
    /// Wall-clock time the feedback was recorded.
    pub recorded_at: DateTime<Utc>,
    /// Optional free-text context that explains the feedback.
    pub context_text: Option<String>,
}

/// Session-scoped, in-memory store of per-memory feedback records.
///
/// Records are appended in order.  When multiple signals exist for the same
/// memory, [`signal_for`] returns the **most recent** one.
///
/// [`signal_for`]: MemoryFeedbackStore::signal_for
#[derive(Debug, Default, Clone)]
pub struct MemoryFeedbackStore {
    records: Vec<MemoryFeedbackRecord>,
}

impl MemoryFeedbackStore {
    /// Create an empty feedback store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a feedback signal for `memory_id`.
    ///
    /// Appends a new [`MemoryFeedbackRecord`] with the current UTC timestamp.
    /// `context` is an optional free-text explanation stored alongside.
    pub fn record(
        &mut self,
        memory_id: &str,
        signal: FeedbackSignal,
        context: Option<&str>,
        now: DateTime<Utc>,
    ) {
        self.records.push(MemoryFeedbackRecord {
            memory_id: memory_id.to_owned(),
            signal,
            recorded_at: now,
            context_text: context.map(str::to_owned),
        });
    }

    /// Return the **most recent** feedback signal recorded for `memory_id`,
    /// or `None` if no feedback has been recorded.
    #[must_use]
    pub fn signal_for(&self, memory_id: &str) -> Option<&FeedbackSignal> {
        self.records
            .iter()
            .rev()
            .find(|r| r.memory_id == memory_id)
            .map(|r| &r.signal)
    }

    /// Return the IDs of all memories whose latest signal is [`FeedbackSignal::Suppress`].
    #[must_use]
    pub fn all_suppressed(&self) -> Vec<&str> {
        self.latest_with_signal(FeedbackSignal::Suppress)
    }

    /// Return the IDs of all memories whose latest signal is [`FeedbackSignal::Promote`].
    #[must_use]
    pub fn all_promoted(&self) -> Vec<&str> {
        self.latest_with_signal(FeedbackSignal::Promote)
    }

    /// Return the IDs of all memories whose latest signal is [`FeedbackSignal::Wrong`].
    #[must_use]
    pub fn all_wrong(&self) -> Vec<&str> {
        self.latest_with_signal(FeedbackSignal::Wrong)
    }

    /// Iterate over all raw feedback records in insertion order.
    #[must_use]
    pub fn records(&self) -> &[MemoryFeedbackRecord] {
        &self.records
    }

    /// Helper: collect distinct memory IDs whose **latest** signal equals `target`.
    fn latest_with_signal(&self, target: FeedbackSignal) -> Vec<&str> {
        // Walk backwards; the first record we encounter for each ID is the latest.
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for record in self.records.iter().rev() {
            if seen.insert(record.memory_id.as_str()) {
                if record.signal == target {
                    result.push(record.memory_id.as_str());
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn ts() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn signal_for_returns_most_recent() {
        let mut store = MemoryFeedbackStore::new();
        store.record("m1", FeedbackSignal::Helpful, None, ts());
        store.record("m1", FeedbackSignal::Wrong, None, ts());
        assert_eq!(store.signal_for("m1"), Some(&FeedbackSignal::Wrong));
    }

    #[test]
    fn all_suppressed_latest_wins() {
        let mut store = MemoryFeedbackStore::new();
        store.record("m1", FeedbackSignal::Suppress, None, ts());
        store.record("m2", FeedbackSignal::Suppress, None, ts());
        // Override m1 with Promote — should not appear in suppressed list
        store.record("m1", FeedbackSignal::Promote, None, ts());
        let suppressed = store.all_suppressed();
        assert!(!suppressed.contains(&"m1"), "m1 was overridden to Promote");
        assert!(suppressed.contains(&"m2"), "m2 is still Suppress");
    }

    #[test]
    fn all_promoted_basic() {
        let mut store = MemoryFeedbackStore::new();
        store.record("m3", FeedbackSignal::Promote, None, ts());
        assert!(store.all_promoted().contains(&"m3"));
    }

    #[test]
    fn context_text_stored() {
        let mut store = MemoryFeedbackStore::new();
        store.record("m4", FeedbackSignal::Irrelevant, Some("off-topic"), ts());
        let record = store.records().last().unwrap();
        assert_eq!(record.context_text.as_deref(), Some("off-topic"));
    }
}
