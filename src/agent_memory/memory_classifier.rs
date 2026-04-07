use super::enums::MemoryType;
use super::schemas::CandidateMemory;

const PREFERENCE_HINTS: &[&str] = &[
    "prefer",
    "preference",
    "favorite",
    "likes",
    "dislikes",
    "theme",
];
const GOAL_HINTS: &[&str] = &[
    "goal",
    "task",
    "status",
    "todo",
    "next_step",
    "milestone",
    "blocked",
];
const EVENT_HINTS: &[&str] = &[
    "met",
    "went",
    "moved",
    "happened",
    "yesterday",
    "today",
    "last",
    "completed",
    "failed",
    "finished",
    "started",
];

/// Deterministic rule-based classifier.
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryClassifier;

impl MemoryClassifier {
    #[must_use]
    pub fn classify(&self, mut candidate: CandidateMemory) -> CandidateMemory {
        // Use the slot string only if one was actually asserted.
        let slot = candidate.slot_non_empty().unwrap_or("").to_lowercase();
        let text = candidate.raw_text.to_lowercase();

        // Only classify as structured fact/preference/goal when all three fields are present.
        // Absent entity, slot, or value means the input lacks enough structure for promotion
        // to those layers; prefer under-classification over fabricating certainty.
        candidate.memory_type = if candidate.entity_non_empty().is_some()
            && candidate.slot_non_empty().is_some()
            && candidate.value_non_empty().is_some()
        {
            if PREFERENCE_HINTS.iter().any(|hint| slot.contains(hint)) {
                MemoryType::Preference
            } else if GOAL_HINTS.iter().any(|hint| slot.contains(hint)) {
                MemoryType::GoalState
            } else {
                MemoryType::Fact
            }
        } else {
            // No complete entity/slot/value triple — check raw text for event signals.
            // Require at least two distinct event-hint words to avoid single-word false
            // positives (e.g. the word "last" alone is not evidence of an episodic event).
            let event_signal_count = EVENT_HINTS
                .iter()
                .filter(|hint| text.contains(*hint))
                .count();
            if event_signal_count >= 2 {
                MemoryType::Episode
            } else {
                MemoryType::Trace
            }
        };

        match candidate.memory_type {
            MemoryType::Preference => {
                candidate.salience = candidate.salience.max(0.9);
                candidate.confidence = candidate.confidence.max(0.7);
            }
            MemoryType::GoalState => {
                candidate.salience = candidate.salience.max(0.85);
                candidate.confidence = candidate.confidence.max(0.65);
            }
            MemoryType::Fact => {
                candidate.salience = candidate.salience.max(0.7);
            }
            MemoryType::Episode => {
                candidate.salience = candidate.salience.max(0.55);
            }
            MemoryType::Trace => {}
        }

        // Structured candidates (entity+slot+value present) may also carry episodic signals
        // in their raw text. Allow upgrading Fact→Episode only when the raw text contains
        // multiple strong event markers and no structured slot name was provided that maps
        // to a belief or preference — this avoids turning "I did X" facts into orphan episodes.
        if candidate.memory_type == MemoryType::Trace {
            let trace_event_signals = ["did ", "ran ", "completed "]
                .iter()
                .filter(|marker| text.contains(*marker))
                .count();
            if trace_event_signals >= 2 {
                candidate.memory_type = MemoryType::Episode;
                candidate.salience = candidate.salience.max(0.55);
            }
        }

        candidate
    }
}
