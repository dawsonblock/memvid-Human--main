use super::enums::MemoryType;
use super::schemas::RetentionRule;

const DAY_SECONDS: i64 = 86_400;

/// Deterministic default policy values for governed memory.
#[derive(Debug, Clone)]
pub struct PolicySet {
    reject: f32,
    trace_only: f32,
    promote: f32,
}

impl Default for PolicySet {
    fn default() -> Self {
        Self {
            reject: 0.25,
            trace_only: 0.35,
            promote: 0.65,
        }
    }
}

impl PolicySet {
    #[must_use]
    pub fn promotion_score(confidence: f32, salience: f32) -> f32 {
        (confidence.clamp(0.0, 1.0) * 0.6) + (salience.clamp(0.0, 1.0) * 0.4)
    }

    #[must_use]
    pub fn reject_threshold(&self) -> f32 {
        self.reject
    }

    #[must_use]
    pub fn store_trace_threshold(&self) -> f32 {
        self.trace_only
    }

    #[must_use]
    pub fn promote_threshold(&self, memory_type: MemoryType) -> f32 {
        match memory_type {
            MemoryType::Fact => 0.75,
            MemoryType::Preference => 0.70,
            MemoryType::GoalState => 0.65,
            MemoryType::Episode => self.promote,
            MemoryType::Trace => 1.1,
        }
    }

    #[must_use]
    pub fn retention_rule(&self, memory_type: MemoryType) -> RetentionRule {
        match memory_type {
            MemoryType::Trace => RetentionRule {
                memory_type,
                default_ttl: Some(3 * DAY_SECONDS),
                decay_per_day: 0.18,
                retrieval_priority: 0.1,
                promotable: false,
            },
            MemoryType::Episode => RetentionRule {
                memory_type,
                default_ttl: Some(30 * DAY_SECONDS),
                decay_per_day: 0.04,
                retrieval_priority: 0.45,
                promotable: true,
            },
            MemoryType::Fact => RetentionRule {
                memory_type,
                default_ttl: None,
                decay_per_day: 0.005,
                retrieval_priority: 0.75,
                promotable: true,
            },
            MemoryType::Preference => RetentionRule {
                memory_type,
                default_ttl: None,
                decay_per_day: 0.002,
                retrieval_priority: 1.0,
                promotable: true,
            },
            MemoryType::GoalState => RetentionRule {
                memory_type,
                default_ttl: Some(14 * DAY_SECONDS),
                decay_per_day: 0.03,
                retrieval_priority: 0.95,
                promotable: true,
            },
        }
    }
}
