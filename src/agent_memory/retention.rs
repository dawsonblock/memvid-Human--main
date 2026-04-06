use chrono::{DateTime, Utc};

use super::policy::PolicySet;
use super::schemas::{DurableMemory, RetentionRule};

/// Retention evaluation output.
#[derive(Debug, Clone, PartialEq)]
pub struct RetentionEvaluation {
    pub rule: RetentionRule,
    pub expired: bool,
    pub decayed_salience: f32,
}

/// Deterministic retention and decay engine.
#[derive(Debug, Clone, Default)]
pub struct RetentionManager {
    policy: PolicySet,
}

impl RetentionManager {
    #[must_use]
    pub fn new(policy: PolicySet) -> Self {
        Self { policy }
    }

    #[must_use]
    pub fn evaluate(&self, memory: &DurableMemory, now: DateTime<Utc>) -> RetentionEvaluation {
        let rule = self
            .policy
            .retention_rule(memory.memory_layer(), memory.memory_type);
        let expired = memory
            .ttl
            .or(rule.default_ttl)
            .is_some_and(|ttl| memory.stored_at.timestamp() + ttl <= now.timestamp());

        let age_days = (now.timestamp() - memory.stored_at.timestamp()).max(0) as f32 / 86_400.0;
        let decay_multiplier = (1.0 - (rule.decay_per_day * age_days)).clamp(0.05, 1.0);

        RetentionEvaluation {
            rule,
            expired,
            decayed_salience: (memory.salience * decay_multiplier).clamp(0.0, 1.0),
        }
    }
}
