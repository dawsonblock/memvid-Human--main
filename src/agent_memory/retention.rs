use chrono::{DateTime, Utc};

use super::enums::{MemoryLayer, ProcedureStatus};
use super::policy::{PolicyProfile, PolicySet};
use super::procedure_store::effective_procedure_status;
use super::schemas::{DurableMemory, ProcedureRecord, RetentionRule};

const DAY_SECONDS: i64 = 86_400;
const LONG_LIVED_PROCEDURE_TTL: i64 = 180 * DAY_SECONDS;
const COOLING_PROCEDURE_TTL: i64 = 45 * DAY_SECONDS;
const RETIRED_PROCEDURE_TTL: i64 = 14 * DAY_SECONDS;
const ACCESS_COUNT_CAP: u32 = 8;
const ACCESS_COUNT_MAX_BOOST: f32 = 0.15;
const ACCESS_RECENCY_WINDOW_DAYS: f32 = 14.0;
const ACCESS_RECENCY_MAX_BOOST: f32 = 0.10;
const OUTCOME_IMPACT_COUNT_CAP: u32 = 6;
const OUTCOME_IMPACT_WINDOW_DAYS: f32 = 30.0;
const OUTCOME_IMPACT_MAX_ADJUSTMENT: f32 = 0.18;

/// Retention evaluation output.
#[derive(Debug, Clone, PartialEq)]
pub struct RetentionEvaluation {
    pub rule: RetentionRule,
    pub expired: bool,
    pub base_salience: f32,
    pub access_boost: f32,
    pub outcome_impact_adjustment: f32,
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
    pub fn policy_profile(&self) -> PolicyProfile {
        self.policy.policy_profile()
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
        let base_salience = (memory.salience * decay_multiplier).clamp(0.0, 1.0);
        let access_boost = Self::access_boost(memory, now);
        let outcome_impact_adjustment = Self::outcome_impact_adjustment(memory, now);

        RetentionEvaluation {
            rule,
            expired,
            base_salience,
            access_boost,
            outcome_impact_adjustment,
            decayed_salience: (base_salience + access_boost + outcome_impact_adjustment)
                .clamp(0.0, 1.0),
        }
    }

    fn access_boost(memory: &DurableMemory, now: DateTime<Utc>) -> f32 {
        let retrieval_count = memory.retrieval_count().min(ACCESS_COUNT_CAP);
        let count_boost = if retrieval_count == 0 {
            0.0
        } else {
            retrieval_count as f32 / ACCESS_COUNT_CAP as f32 * ACCESS_COUNT_MAX_BOOST
        };

        let recency_boost = memory.last_accessed_at().map_or(0.0, |last_accessed_at| {
            let age_days =
                (now.timestamp() - last_accessed_at.timestamp()).max(0) as f32 / DAY_SECONDS as f32;
            (1.0 - (age_days / ACCESS_RECENCY_WINDOW_DAYS)).clamp(0.0, 1.0)
                * ACCESS_RECENCY_MAX_BOOST
        });

        (count_boost + recency_boost).clamp(0.0, ACCESS_COUNT_MAX_BOOST + ACCESS_RECENCY_MAX_BOOST)
    }

    fn outcome_impact_adjustment(memory: &DurableMemory, now: DateTime<Utc>) -> f32 {
        let positive = memory.positive_outcome_count();
        let negative = memory.negative_outcome_count();
        let total = positive + negative;
        if total == 0 {
            return 0.0;
        }

        let count_weight =
            total.min(OUTCOME_IMPACT_COUNT_CAP) as f32 / OUTCOME_IMPACT_COUNT_CAP as f32;
        let recency_weight = memory.last_outcome_at().map_or(0.0, |last_outcome_at| {
            let age_days =
                (now.timestamp() - last_outcome_at.timestamp()).max(0) as f32 / DAY_SECONDS as f32;
            (1.0 - (age_days / OUTCOME_IMPACT_WINDOW_DAYS)).clamp(0.0, 1.0)
        });

        memory.outcome_impact_score()
            * count_weight
            * recency_weight
            * OUTCOME_IMPACT_MAX_ADJUSTMENT
    }

    fn adjust_procedure_rule(rule: &mut RetentionRule, record: &ProcedureRecord) {
        let effective_status = effective_procedure_status(record);
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
}
