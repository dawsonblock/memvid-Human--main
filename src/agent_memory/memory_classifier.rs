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
];

/// Deterministic rule-based classifier.
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryClassifier;

impl MemoryClassifier {
    #[must_use]
    pub fn classify(&self, mut candidate: CandidateMemory) -> CandidateMemory {
        let slot = candidate.slot.to_lowercase();
        let text = candidate.raw_text.to_lowercase();

        candidate.memory_type = if !candidate.entity.trim().is_empty()
            && !candidate.slot.trim().is_empty()
            && !candidate.value.trim().is_empty()
        {
            if PREFERENCE_HINTS.iter().any(|hint| slot.contains(hint)) {
                MemoryType::Preference
            } else if GOAL_HINTS.iter().any(|hint| slot.contains(hint)) {
                MemoryType::GoalState
            } else {
                MemoryType::Fact
            }
        } else if EVENT_HINTS.iter().any(|hint| text.contains(hint)) {
            MemoryType::Episode
        } else {
            MemoryType::Trace
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

        if candidate.memory_type == MemoryType::Trace
            && (text.contains("did") || text.contains("ran") || text.contains("completed"))
        {
            candidate.memory_type = MemoryType::Episode;
            candidate.salience = candidate.salience.max(0.55);
        }

        candidate
    }
}
