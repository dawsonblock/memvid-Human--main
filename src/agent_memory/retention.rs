use chrono::{DateTime, Utc};

use super::enums::{MemoryLayer, ProcedureStatus};
use super::policy::PolicySet;
use super::schemas::{DurableMemory, ProcedureRecord, RetentionRule};

const DAY_SECONDS: i64 = 86_400;
const LONG_LIVED_PROCEDURE_TTL: i64 = 180 * DAY_SECONDS;
const COOLING_PROCEDURE_TTL: i64 = 45 * DAY_SECONDS;
const RETIRED_PROCEDURE_TTL: i64 = 14 * DAY_SECONDS;

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
        let mut rule = self
            .policy
            .retention_rule(memory.memory_layer(), memory.memory_type);
        let mut age_anchor = memory.stored_at;

        if memory.memory_layer() == MemoryLayer::Procedure
            && let Some(record) = memory.to_procedure_record()
        {
            if let Some(last_activity) = record.last_used_at.or(record.last_succeeded_at)
                && last_activity > age_anchor
            {
                age_anchor = last_activity;
            }
            Self::adjust_procedure_rule(&mut rule, &record);
        }

        let expired = memory
            .ttl
            .or(rule.default_ttl)
            .is_some_and(|ttl| age_anchor.timestamp() + ttl <= now.timestamp());

        let age_days =
            (now.timestamp() - age_anchor.timestamp()).max(0) as f32 / DAY_SECONDS as f32;
        let decay_multiplier = (1.0 - (rule.decay_per_day * age_days)).clamp(0.05, 1.0);

        RetentionEvaluation {
            rule,
            expired,
            decayed_salience: (memory.salience * decay_multiplier).clamp(0.0, 1.0),
        }
    }

    fn adjust_procedure_rule(rule: &mut RetentionRule, record: &ProcedureRecord) {
        let effective_status = Self::effective_procedure_status(record);
        let total_runs = record.success_count + record.failure_count;
        let failure_ratio = if total_runs == 0 {
            0.0
        } else {
            record.failure_count as f32 / total_runs as f32
        };

        match effective_status {
            ProcedureStatus::Active => {
                if record.success_count > record.failure_count {
                    rule.default_ttl = Some(
                        rule.default_ttl
                            .unwrap_or(LONG_LIVED_PROCEDURE_TTL)
                            .max(LONG_LIVED_PROCEDURE_TTL),
                    );
                    rule.decay_per_day *= 0.6;
                    rule.retrieval_priority += 0.15;
                } else {
                    rule.decay_per_day *= 0.85;
                }
            }
            ProcedureStatus::CoolingDown => {
                rule.default_ttl = Some(
                    rule.default_ttl
                        .unwrap_or(COOLING_PROCEDURE_TTL)
                        .min(COOLING_PROCEDURE_TTL),
                );
                rule.decay_per_day *= 2.2;
                rule.retrieval_priority *= 0.55;
            }
            ProcedureStatus::Retired => {
                rule.default_ttl = Some(RETIRED_PROCEDURE_TTL);
                rule.decay_per_day *= 4.0;
                rule.retrieval_priority = 0.0;
            }
        }

        if failure_ratio >= 0.7 && total_runs >= 4 {
            rule.decay_per_day *= 1.35;
        }
    }

    fn effective_procedure_status(record: &ProcedureRecord) -> ProcedureStatus {
        if record.status == ProcedureStatus::Retired {
            return ProcedureStatus::Retired;
        }
        if record.status == ProcedureStatus::CoolingDown {
            return ProcedureStatus::CoolingDown;
        }

        let total_runs = record.success_count + record.failure_count;
        if total_runs >= 5 && record.failure_count >= record.success_count.saturating_add(3) {
            ProcedureStatus::Retired
        } else if total_runs >= 3 && record.failure_count > record.success_count {
            ProcedureStatus::CoolingDown
        } else {
            ProcedureStatus::Active
        }
    }
}
