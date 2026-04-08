use std::collections::BTreeMap;

use uuid::Uuid;

use super::clock::Clock;
use super::enums::{BeliefAction, BeliefStatus};
use super::schemas::{BeliefRecord, DurableMemory};

/// Result of belief mutation.
#[derive(Debug, Clone)]
pub struct BeliefUpdateOutcome {
    pub action: BeliefAction,
    pub current_belief: Option<BeliefRecord>,
    pub prior_belief: Option<BeliefRecord>,
}

/// Deterministic explicit belief updater.
#[derive(Debug, Default, Clone, Copy)]
pub struct BeliefUpdater;

impl BeliefUpdater {
    #[must_use]
    pub fn apply(
        &self,
        existing: Option<BeliefRecord>,
        memory: &DurableMemory,
        clock: &dyn Clock,
    ) -> BeliefUpdateOutcome {
        let now = clock.now();

        match existing {
            None => {
                let mut source_weights = BTreeMap::new();
                source_weights.insert(memory.source.source_type, memory.source.trust_weight);
                BeliefUpdateOutcome {
                    action: if memory.is_retraction {
                        BeliefAction::Retract
                    } else {
                        BeliefAction::Update
                    },
                    current_belief: Some(BeliefRecord {
                        belief_id: Uuid::new_v4().to_string(),
                        entity: memory.entity.clone(),
                        slot: memory.slot.clone(),
                        current_value: memory.value.clone(),
                        status: if memory.is_retraction {
                            BeliefStatus::Retracted
                        } else {
                            BeliefStatus::Active
                        },
                        confidence: memory.confidence,
                        valid_from: memory.valid_from.unwrap_or(memory.stored_at),
                        valid_to: if memory.is_retraction {
                            Some(now)
                        } else {
                            None
                        },
                        last_reviewed_at: now,
                        supporting_memory_ids: if memory.is_retraction {
                            Vec::new()
                        } else {
                            vec![memory.memory_id.clone()]
                        },
                        opposing_memory_ids: Vec::new(),
                        contradictions_observed: 0,
                        last_contradiction_at: None,
                        time_to_last_resolution_seconds: None,
                        positive_outcome_count: 0,
                        negative_outcome_count: 0,
                        last_outcome_at: None,
                        source_weights,
                    }),
                    prior_belief: None,
                }
            }
            Some(mut current) => {
                if memory.is_retraction {
                    current.status = BeliefStatus::Retracted;
                    current.valid_to = Some(now);
                    current.last_reviewed_at = now;
                    if !current.opposing_memory_ids.contains(&memory.memory_id) {
                        current.opposing_memory_ids.push(memory.memory_id.clone());
                    }
                    if let Some(last_contradiction_at) = current.last_contradiction_at {
                        current.time_to_last_resolution_seconds =
                            Some((now.timestamp() - last_contradiction_at.timestamp()).max(0));
                    }
                    current.last_contradiction_at = None;
                    return BeliefUpdateOutcome {
                        action: BeliefAction::Retract,
                        current_belief: Some(current),
                        prior_belief: None,
                    };
                }

                if current.current_value == memory.value {
                    current.confidence = current.confidence.max(memory.confidence);
                    current.last_reviewed_at = now;
                    if !current.supporting_memory_ids.contains(&memory.memory_id) {
                        current.supporting_memory_ids.push(memory.memory_id.clone());
                    }
                    current
                        .source_weights
                        .insert(memory.source.source_type, memory.source.trust_weight);
                    if current.status == BeliefStatus::Disputed {
                        current.status = BeliefStatus::Active;
                        if let Some(last_contradiction_at) = current.last_contradiction_at {
                            current.time_to_last_resolution_seconds =
                                Some((now.timestamp() - last_contradiction_at.timestamp()).max(0));
                        }
                        current.last_contradiction_at = None;
                    }
                    return BeliefUpdateOutcome {
                        action: BeliefAction::Reinforce,
                        current_belief: Some(current),
                        prior_belief: None,
                    };
                }

                let existing_trust = current.strongest_source_weight();
                let new_trust = memory.source.trust_weight;
                let comparable_confidence = memory.confidence + 0.05 >= current.confidence;

                if new_trust > existing_trust && comparable_confidence {
                    let mut stale = current.clone();
                    stale.status = BeliefStatus::Stale;
                    stale.valid_to = Some(now);
                    stale.last_reviewed_at = now;
                    stale.opposing_memory_ids.push(memory.memory_id.clone());

                    let mut source_weights = BTreeMap::new();
                    source_weights.insert(memory.source.source_type, memory.source.trust_weight);
                    let replacement = BeliefRecord {
                        belief_id: Uuid::new_v4().to_string(),
                        entity: memory.entity.clone(),
                        slot: memory.slot.clone(),
                        current_value: memory.value.clone(),
                        status: BeliefStatus::Active,
                        confidence: memory.confidence,
                        valid_from: memory.valid_from.unwrap_or(memory.stored_at),
                        valid_to: None,
                        last_reviewed_at: now,
                        supporting_memory_ids: vec![memory.memory_id.clone()],
                        opposing_memory_ids: stale.supporting_memory_ids.clone(),
                        contradictions_observed: 0,
                        last_contradiction_at: None,
                        time_to_last_resolution_seconds: current
                            .last_contradiction_at
                            .map(|observed_at| (now.timestamp() - observed_at.timestamp()).max(0)),
                        positive_outcome_count: 0,
                        negative_outcome_count: 0,
                        last_outcome_at: None,
                        source_weights,
                    };
                    return BeliefUpdateOutcome {
                        action: BeliefAction::Update,
                        current_belief: Some(replacement),
                        prior_belief: Some(stale),
                    };
                }

                current.status = BeliefStatus::Disputed;
                current.last_reviewed_at = now;
                let mut observed_new_conflict = false;
                if !current.opposing_memory_ids.contains(&memory.memory_id) {
                    current.opposing_memory_ids.push(memory.memory_id.clone());
                    observed_new_conflict = true;
                }
                if observed_new_conflict {
                    current.contradictions_observed =
                        current.contradictions_observed.saturating_add(1);
                }
                current.last_contradiction_at = Some(now);
                current.time_to_last_resolution_seconds = None;
                BeliefUpdateOutcome {
                    action: BeliefAction::Dispute,
                    current_belief: Some(current),
                    prior_belief: None,
                }
            }
        }
    }
}
