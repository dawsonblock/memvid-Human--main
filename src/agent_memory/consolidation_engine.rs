use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use super::adapters::memvid_store::MemoryStore;
use super::clock::Clock;
use super::enums::MemoryLayer;
use super::episode_store::EpisodeStore;
use super::errors::Result;
use super::goal_state_store::GoalStateStore;
use super::policy::PolicySet;
use super::procedure_store::{ProcedureStatusTransition, ProcedureStore};
use super::schemas::{ConsolidationRecord, DurableMemory, RetrievalQuery};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsolidationDisposition {
    Initial,
    Reinforcement,
    Duplicate,
}

/// Consolidation result emitted after repeated bounded patterns are promoted.
#[derive(Debug, Clone)]
pub struct ConsolidationOutcome {
    pub record: ConsolidationRecord,
    pub trace_id: String,
    pub learned_procedure_id: Option<String>,
    pub procedure_status_transition: Option<ProcedureStatusTransition>,
}

/// Bounded consolidation process over recent episodes and durable preferences.
#[derive(Debug, Clone)]
pub struct ConsolidationEngine {
    policy: PolicySet,
}

impl Default for ConsolidationEngine {
    fn default() -> Self {
        Self::new(PolicySet::default())
    }
}

impl ConsolidationEngine {
    #[must_use]
    pub fn new(policy: PolicySet) -> Self {
        Self { policy }
    }

    pub fn consolidate<S: MemoryStore>(
        &self,
        store: &mut S,
        episode_memory: Option<&DurableMemory>,
        primary_memory: Option<&DurableMemory>,
        clock: &dyn Clock,
    ) -> Result<Vec<ConsolidationOutcome>> {
        let now = clock.now();
        let window_start = now - Duration::days(self.policy.consolidation_window_days());
        let mut outcomes = Vec::new();

        if let Some(memory) = primary_memory {
            if let Some(outcome) = self.self_model_outcome(store, memory, window_start, now)? {
                outcomes.push(outcome);
            }
            if let Some(outcome) = self.belief_window_outcome(store, memory, window_start, now)? {
                outcomes.push(outcome);
            }
            if let Some(outcome) = self.blocker_outcome(store, memory, window_start, now)? {
                outcomes.push(outcome);
            }
        }

        if let Some(episode) = episode_memory
            && let Some(workflow_key) = episode.metadata.get("workflow_key")
        {
            let outcome_value = episode.metadata.get("outcome").map(String::as_str);
            if Self::is_success_outcome(outcome_value) {
                let successful_episodes = {
                    let mut episode_store = EpisodeStore::new(store);
                    episode_store
                        .list_by_workflow_key(workflow_key)?
                        .into_iter()
                        .filter(|record| record.event_at >= window_start)
                        .filter(|record| Self::is_success_outcome(record.outcome.as_deref()))
                        .collect::<Vec<_>>()
                };

                if successful_episodes.len() >= self.policy.minimum_procedure_success_repetitions()
                {
                    let source_memory_ids: Vec<_> = successful_episodes
                        .iter()
                        .map(|record| record.memory_id.clone())
                        .collect();
                    let semantic_key = self.semantic_key(
                        MemoryLayer::Procedure,
                        "procedure_success",
                        [&workflow_key[..]],
                    );
                    let fingerprint = self.evidence_fingerprint(
                        &semantic_key,
                        &source_memory_ids,
                        &[self.policy.consolidation_window_days().to_string()],
                    );
                    let disposition =
                        self.consolidation_disposition(store, &semantic_key, &fingerprint)?;
                    if disposition != ConsolidationDisposition::Duplicate {
                        let description = episode
                                    .metadata
                                    .get("procedure_description")
                                    .cloned()
                                    .unwrap_or_else(|| {
                                        format!(
                                            "workflow {workflow_key} has succeeded repeatedly and should be reused"
                                        )
                                    });
                        let procedure_outcome = {
                            let mut procedure_store = ProcedureStore::new(store);
                            procedure_store.upsert_success(
                                workflow_key,
                                &description,
                                &source_memory_ids,
                                now,
                            )?
                        };
                        let action = match disposition {
                            ConsolidationDisposition::Initial => "promotion",
                            ConsolidationDisposition::Reinforcement => "reinforcement",
                            ConsolidationDisposition::Duplicate => unreachable!(),
                        };
                        let record = ConsolidationRecord {
                            consolidation_id: Uuid::new_v4().to_string(),
                            target_layer: MemoryLayer::Procedure,
                            target_id: Some(procedure_outcome.record.procedure_id.clone()),
                            source_memory_ids,
                            reason: if disposition == ConsolidationDisposition::Initial {
                                format!(
                                    "repeated successful workflow {workflow_key} promoted into procedure memory"
                                )
                            } else {
                                format!(
                                    "repeated successful workflow {workflow_key} reinforced learned procedure memory"
                                )
                            },
                            confidence: procedure_outcome.record.confidence,
                            created_at: now,
                            metadata: {
                                let mut metadata = BTreeMap::from([
                                    ("workflow_key".to_string(), workflow_key.clone()),
                                    ("outcome".to_string(), "success".to_string()),
                                    ("consolidation_action".to_string(), action.to_string()),
                                    ("consolidation_semantic_key".to_string(), semantic_key),
                                    ("consolidation_fingerprint".to_string(), fingerprint),
                                    (
                                        "window_days".to_string(),
                                        self.policy.consolidation_window_days().to_string(),
                                    ),
                                    (
                                        "minimum_repetitions".to_string(),
                                        self.policy
                                            .minimum_procedure_success_repetitions()
                                            .to_string(),
                                    ),
                                ]);
                                if let Some(transition) = &procedure_outcome.status_transition {
                                    metadata.insert(
                                        "previous_procedure_status".to_string(),
                                        transition.previous_status.as_str().to_string(),
                                    );
                                    metadata.insert(
                                        "next_procedure_status".to_string(),
                                        transition.next_status.as_str().to_string(),
                                    );
                                }
                                metadata
                            },
                        };
                        let trace_id = self.persist_record(store, &record)?;
                        outcomes.push(ConsolidationOutcome {
                            record,
                            trace_id,
                            learned_procedure_id: Some(procedure_outcome.record.procedure_id),
                            procedure_status_transition: procedure_outcome.status_transition,
                        });
                    }
                }
            } else if Self::is_failure_outcome(outcome_value) {
                let failure_memory_ids = vec![episode.memory_id.clone()];
                let semantic_key = self.semantic_key(
                    MemoryLayer::Procedure,
                    "procedure_failure",
                    [&workflow_key[..]],
                );
                let fingerprint =
                    self.evidence_fingerprint(&semantic_key, &failure_memory_ids, &[]);
                let disposition =
                    self.consolidation_disposition(store, &semantic_key, &fingerprint)?;
                if disposition != ConsolidationDisposition::Duplicate {
                    let failure_outcome = {
                        let mut procedure_store = ProcedureStore::new(store);
                        procedure_store.record_failure(workflow_key, &failure_memory_ids, now)?
                    };

                    if let Some(procedure_outcome) = failure_outcome {
                        let action = match disposition {
                            ConsolidationDisposition::Initial => "promotion",
                            ConsolidationDisposition::Reinforcement => "reinforcement",
                            ConsolidationDisposition::Duplicate => unreachable!(),
                        };
                        let record = ConsolidationRecord {
                            consolidation_id: Uuid::new_v4().to_string(),
                            target_layer: MemoryLayer::Procedure,
                            target_id: Some(procedure_outcome.record.procedure_id.clone()),
                            source_memory_ids: failure_memory_ids,
                            reason: if disposition == ConsolidationDisposition::Initial {
                                format!(
                                    "observed workflow failure for {workflow_key} updated procedure lifecycle"
                                )
                            } else {
                                format!(
                                    "observed workflow failure for {workflow_key} reinforced procedure lifecycle degradation"
                                )
                            },
                            confidence: procedure_outcome.record.confidence,
                            created_at: now,
                            metadata: {
                                let mut metadata = BTreeMap::from([
                                    ("workflow_key".to_string(), workflow_key.clone()),
                                    ("outcome".to_string(), "failure".to_string()),
                                    ("consolidation_action".to_string(), action.to_string()),
                                    ("consolidation_semantic_key".to_string(), semantic_key),
                                    ("consolidation_fingerprint".to_string(), fingerprint),
                                ]);
                                if let Some(transition) = &procedure_outcome.status_transition {
                                    metadata.insert(
                                        "previous_procedure_status".to_string(),
                                        transition.previous_status.as_str().to_string(),
                                    );
                                    metadata.insert(
                                        "next_procedure_status".to_string(),
                                        transition.next_status.as_str().to_string(),
                                    );
                                }
                                metadata
                            },
                        };
                        let trace_id = self.persist_record(store, &record)?;
                        outcomes.push(ConsolidationOutcome {
                            record,
                            trace_id,
                            learned_procedure_id: None,
                            procedure_status_transition: procedure_outcome.status_transition,
                        });
                    }
                }
            }
        }

        Ok(outcomes)
    }

    fn self_model_outcome<S: MemoryStore>(
        &self,
        store: &mut S,
        memory: &DurableMemory,
        window_start: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Option<ConsolidationOutcome>> {
        if memory.memory_layer() != MemoryLayer::SelfModel || memory.is_retraction {
            return Ok(None);
        }

        let matching = store
            .list_memory_versions_by_layer(MemoryLayer::SelfModel)?
            .into_iter()
            .filter(|candidate| !candidate.is_retraction)
            .filter(|candidate| candidate.entity == memory.entity)
            .filter(|candidate| candidate.slot == memory.slot)
            .filter(|candidate| candidate.value == memory.value)
            .filter(|candidate| candidate.version_timestamp() >= window_start)
            .collect::<Vec<_>>();
        if matching.len() < self.policy.minimum_self_model_repetitions() {
            return Ok(None);
        }

        let source_memory_ids: Vec<_> = matching
            .into_iter()
            .map(|record| record.memory_id)
            .collect();
        let semantic_key = self.semantic_key(
            MemoryLayer::SelfModel,
            "self_model_stable",
            [&memory.entity[..], &memory.slot[..], &memory.value[..]],
        );
        let fingerprint = self.evidence_fingerprint(
            &semantic_key,
            &source_memory_ids,
            &[
                self.policy.consolidation_window_days().to_string(),
                self.policy.minimum_self_model_repetitions().to_string(),
            ],
        );
        let disposition = self.consolidation_disposition(store, &semantic_key, &fingerprint)?;
        if disposition == ConsolidationDisposition::Duplicate {
            return Ok(None);
        }
        let action = match disposition {
            ConsolidationDisposition::Initial => "promotion",
            ConsolidationDisposition::Reinforcement => "reinforcement",
            ConsolidationDisposition::Duplicate => unreachable!(),
        };

        let record = ConsolidationRecord {
            consolidation_id: Uuid::new_v4().to_string(),
            target_layer: MemoryLayer::SelfModel,
            target_id: Some(memory.memory_id.clone()),
            source_memory_ids,
            reason: if disposition == ConsolidationDisposition::Initial {
                "repeated self-model observations stabilized into durable preference".to_string()
            } else {
                "repeated self-model observations reinforced durable preference".to_string()
            },
            confidence: memory.confidence,
            created_at: now,
            metadata: BTreeMap::from([
                ("entity".to_string(), memory.entity.clone()),
                ("slot".to_string(), memory.slot.clone()),
                ("value".to_string(), memory.value.clone()),
                ("consolidation_action".to_string(), action.to_string()),
                ("consolidation_semantic_key".to_string(), semantic_key),
                ("consolidation_fingerprint".to_string(), fingerprint),
                (
                    "window_days".to_string(),
                    self.policy.consolidation_window_days().to_string(),
                ),
                (
                    "minimum_repetitions".to_string(),
                    self.policy.minimum_self_model_repetitions().to_string(),
                ),
            ]),
        };
        let trace_id = self.persist_record(store, &record)?;
        Ok(Some(ConsolidationOutcome {
            record,
            trace_id,
            learned_procedure_id: None,
            procedure_status_transition: None,
        }))
    }

    fn belief_window_outcome<S: MemoryStore>(
        &self,
        store: &mut S,
        memory: &DurableMemory,
        window_start: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Option<ConsolidationOutcome>> {
        if memory.memory_layer() != MemoryLayer::Belief || memory.is_retraction {
            return Ok(None);
        }

        let mut matching: Vec<_> = store
            .list_memories_for_belief(&memory.entity, &memory.slot)?
            .into_iter()
            .filter(|candidate| !candidate.is_retraction)
            .filter(|candidate| candidate.value == memory.value)
            .filter(|candidate| candidate.event_timestamp() >= window_start)
            .collect();
        matching.sort_by(|left, right| left.event_timestamp().cmp(&right.event_timestamp()));
        if matching.len() < self.policy.minimum_belief_stabilization_repetitions() {
            return Ok(None);
        }

        let span = matching
            .last()
            .zip(matching.first())
            .map(|(latest, earliest)| latest.event_timestamp() - earliest.event_timestamp())
            .unwrap_or_else(Duration::zero);
        if span < Duration::days(self.policy.belief_stability_min_days()) {
            return Ok(None);
        }

        let source_memory_ids: Vec<_> = matching
            .into_iter()
            .map(|candidate| candidate.memory_id)
            .collect();
        let semantic_key = self.semantic_key(
            MemoryLayer::Belief,
            "belief_stable",
            [&memory.entity[..], &memory.slot[..], &memory.value[..]],
        );
        let fingerprint = self.evidence_fingerprint(
            &semantic_key,
            &source_memory_ids,
            &[
                self.policy.consolidation_window_days().to_string(),
                self.policy.belief_stability_min_days().to_string(),
            ],
        );
        let disposition = self.consolidation_disposition(store, &semantic_key, &fingerprint)?;
        if disposition == ConsolidationDisposition::Duplicate {
            return Ok(None);
        }
        let action = match disposition {
            ConsolidationDisposition::Initial => "promotion",
            ConsolidationDisposition::Reinforcement => "reinforcement",
            ConsolidationDisposition::Duplicate => unreachable!(),
        };

        let record = ConsolidationRecord {
            consolidation_id: Uuid::new_v4().to_string(),
            target_layer: MemoryLayer::Belief,
            target_id: Some(memory.memory_id.clone()),
            source_memory_ids,
            reason: if disposition == ConsolidationDisposition::Initial {
                "consistent belief evidence remained stable across a bounded window".to_string()
            } else {
                "consistent belief evidence reinforced a stable bounded belief window".to_string()
            },
            confidence: memory.confidence,
            created_at: now,
            metadata: BTreeMap::from([
                ("entity".to_string(), memory.entity.clone()),
                ("slot".to_string(), memory.slot.clone()),
                ("value".to_string(), memory.value.clone()),
                ("consolidation_action".to_string(), action.to_string()),
                ("consolidation_semantic_key".to_string(), semantic_key),
                ("consolidation_fingerprint".to_string(), fingerprint),
                (
                    "window_days".to_string(),
                    self.policy.consolidation_window_days().to_string(),
                ),
                (
                    "stability_days".to_string(),
                    self.policy.belief_stability_min_days().to_string(),
                ),
                (
                    "minimum_repetitions".to_string(),
                    self.policy
                        .minimum_belief_stabilization_repetitions()
                        .to_string(),
                ),
            ]),
        };
        let trace_id = self.persist_record(store, &record)?;
        Ok(Some(ConsolidationOutcome {
            record,
            trace_id,
            learned_procedure_id: None,
            procedure_status_transition: None,
        }))
    }

    fn blocker_outcome<S: MemoryStore>(
        &self,
        store: &mut S,
        memory: &DurableMemory,
        window_start: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Option<ConsolidationOutcome>> {
        if memory.memory_layer() != MemoryLayer::GoalState || memory.is_retraction {
            return Ok(None);
        }

        let Some(goal) = memory.to_goal_record() else {
            return Ok(None);
        };
        let Some(blocker_key) = GoalStateStore::<S>::blocker_key(&goal) else {
            return Ok(None);
        };

        let matching = store
            .list_memory_versions_by_layer(MemoryLayer::GoalState)?
            .into_iter()
            .filter_map(|candidate| candidate.to_goal_record())
            .filter(|record| record.entity == goal.entity)
            .filter(|record| record.slot == goal.slot)
            .filter(|record| {
                GoalStateStore::<S>::blocker_key(record).as_deref() == Some(blocker_key.as_str())
            })
            .filter(|record| record.updated_at >= window_start)
            .collect::<Vec<_>>();
        if matching.len() < self.policy.minimum_blocker_repetitions() {
            return Ok(None);
        }

        let source_memory_ids: Vec<_> = matching
            .into_iter()
            .map(|record| record.memory_id)
            .collect();
        let semantic_key = self.semantic_key(
            MemoryLayer::GoalState,
            "recurring_blocker",
            [&goal.entity[..], &goal.slot[..], &blocker_key[..]],
        );
        let fingerprint = self.evidence_fingerprint(
            &semantic_key,
            &source_memory_ids,
            &[
                self.policy.consolidation_window_days().to_string(),
                self.policy.minimum_blocker_repetitions().to_string(),
            ],
        );
        let disposition = self.consolidation_disposition(store, &semantic_key, &fingerprint)?;
        if disposition == ConsolidationDisposition::Duplicate {
            return Ok(None);
        }
        let action = match disposition {
            ConsolidationDisposition::Initial => "promotion",
            ConsolidationDisposition::Reinforcement => "reinforcement",
            ConsolidationDisposition::Duplicate => unreachable!(),
        };

        let record = ConsolidationRecord {
            consolidation_id: Uuid::new_v4().to_string(),
            target_layer: MemoryLayer::GoalState,
            target_id: Some(goal.goal_id.clone()),
            source_memory_ids,
            reason: if disposition == ConsolidationDisposition::Initial {
                format!("recurring blocker pattern stabilized for {}", goal.slot)
            } else {
                format!("recurring blocker pattern reinforced for {}", goal.slot)
            },
            confidence: memory.confidence,
            created_at: now,
            metadata: BTreeMap::from([
                ("entity".to_string(), goal.entity),
                ("slot".to_string(), goal.slot),
                ("blocker_key".to_string(), blocker_key),
                ("consolidation_action".to_string(), action.to_string()),
                ("consolidation_semantic_key".to_string(), semantic_key),
                ("consolidation_fingerprint".to_string(), fingerprint),
                (
                    "window_days".to_string(),
                    self.policy.consolidation_window_days().to_string(),
                ),
                (
                    "threshold".to_string(),
                    self.policy.minimum_blocker_repetitions().to_string(),
                ),
            ]),
        };
        let trace_id = self.persist_record(store, &record)?;
        Ok(Some(ConsolidationOutcome {
            record,
            trace_id,
            learned_procedure_id: None,
            procedure_status_transition: None,
        }))
    }

    fn persist_record<S: MemoryStore>(
        &self,
        store: &mut S,
        record: &ConsolidationRecord,
    ) -> Result<String> {
        let raw_text = serde_json::to_string(record)?;
        let metadata = BTreeMap::from([
            (
                "consolidation_id".to_string(),
                record.consolidation_id.clone(),
            ),
            (
                "target_layer".to_string(),
                record.target_layer.as_str().to_string(),
            ),
            ("reason".to_string(), record.reason.clone()),
            (
                "consolidation_action".to_string(),
                record
                    .metadata
                    .get("consolidation_action")
                    .cloned()
                    .unwrap_or_else(|| "promotion".to_string()),
            ),
            (
                "consolidation_semantic_key".to_string(),
                record
                    .metadata
                    .get("consolidation_semantic_key")
                    .cloned()
                    .unwrap_or_default(),
            ),
            (
                "consolidation_fingerprint".to_string(),
                record
                    .metadata
                    .get("consolidation_fingerprint")
                    .cloned()
                    .unwrap_or_default(),
            ),
        ]);
        store.put_trace(&raw_text, metadata)
    }

    fn semantic_key<'a, I>(&self, layer: MemoryLayer, reason_key: &str, identity: I) -> String
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut raw = format!("{}|{}", layer.as_str(), reason_key);
        for part in identity {
            raw.push('|');
            raw.push_str(part);
        }
        format!("consolidation-semantic-{:016x}", Self::stable_hash(&raw))
    }

    fn evidence_fingerprint(
        &self,
        semantic_key: &str,
        source_memory_ids: &[String],
        extras: &[String],
    ) -> String {
        let mut source_memory_ids = source_memory_ids.to_vec();
        source_memory_ids.sort();
        let mut raw = format!("{semantic_key}|{}", source_memory_ids.join(","));
        if !extras.is_empty() {
            raw.push('|');
            raw.push_str(&extras.join("|"));
        }
        format!("consolidation-fingerprint-{:016x}", Self::stable_hash(&raw))
    }

    fn consolidation_disposition<S: MemoryStore>(
        &self,
        store: &mut S,
        semantic_key: &str,
        fingerprint: &str,
    ) -> Result<ConsolidationDisposition> {
        let hits = store.search(&RetrievalQuery {
            query_text: semantic_key.to_string(),
            intent: super::enums::QueryIntent::SemanticBackground,
            entity: None,
            slot: None,
            scope: None,
            top_k: 128,
            as_of: None,
            include_expired: true,
        })?;
        let mut saw_semantic_match = false;
        for hit in hits {
            if hit.memory_layer != Some(MemoryLayer::Trace) {
                continue;
            }
            if hit
                .metadata
                .get("consolidation_semantic_key")
                .map(String::as_str)
                != Some(semantic_key)
            {
                continue;
            }
            if hit.metadata.get("action").map(String::as_str) == Some("procedure_status_changed") {
                continue;
            }
            saw_semantic_match = true;
            if hit
                .metadata
                .get("consolidation_fingerprint")
                .map(String::as_str)
                == Some(fingerprint)
            {
                return Ok(ConsolidationDisposition::Duplicate);
            }
        }

        Ok(if saw_semantic_match {
            ConsolidationDisposition::Reinforcement
        } else {
            ConsolidationDisposition::Initial
        })
    }

    fn stable_hash(value: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    fn is_success_outcome(value: Option<&str>) -> bool {
        value.is_some_and(|text| {
            let lower = text.to_lowercase();
            lower.contains("success")
                || lower.contains("completed")
                || lower.contains("passed")
                || lower.contains("ok")
        })
    }

    fn is_failure_outcome(value: Option<&str>) -> bool {
        value.is_some_and(|text| {
            let lower = text.to_lowercase();
            lower.contains("fail")
                || lower.contains("error")
                || lower.contains("blocked")
                || lower.contains("aborted")
                || lower.contains("timeout")
        })
    }
}
