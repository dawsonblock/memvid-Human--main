use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};

use super::adapters::memvid_store::MemoryStore;
use super::clock::Clock;
use super::consolidation_engine::ConsolidationEngine;
use super::enums::MemoryLayer;
use super::errors::Result;
use super::reflection_governance::ReflectionCandidate;

/// Alert emitted when observed episodes diverge from the active belief value.
#[derive(Debug, Clone)]
pub struct BeliefDriftAlert {
    pub belief_id: String,
    pub entity: String,
    pub slot: String,
    pub current_value: String,
    pub contradicting_episode_count: usize,
    pub alert_text: String,
}

/// A recurring goal that has accumulated failure outcomes.
#[derive(Debug, Clone)]
pub struct GoalPattern {
    pub goal_text: String,
    pub failure_count: usize,
    pub last_seen: DateTime<Utc>,
}

/// Full output of a single reasoning cycle.
#[derive(Debug, Clone)]
pub struct ReasoningCycleResult {
    pub reflections: Vec<String>,
    /// Structured candidates ready for governance validation and persistence.
    pub reflect_candidates: Vec<ReflectionCandidate>,
    pub drift_alerts: Vec<BeliefDriftAlert>,
    pub goal_patterns: Vec<GoalPattern>,
    pub promoted_procedures: Vec<String>,
    pub cycle_at: DateTime<Utc>,
}

/// Active reasoning loop: reflects on recent episodes, detects belief drift, tracks goal
/// failures, and delegates procedural promotion to the consolidation engine.
pub struct ReasoningEngine {
    consolidation: ConsolidationEngine,
}

impl ReasoningEngine {
    #[must_use]
    pub fn new(consolidation: ConsolidationEngine) -> Self {
        Self { consolidation }
    }

    pub fn run_cycle<S: MemoryStore>(
        &self,
        store: &mut S,
        clock: &dyn Clock,
    ) -> Result<ReasoningCycleResult> {
        let now = clock.now();
        let cutoff = now - Duration::hours(24);

        // ── Reflection: slot-frequency analysis over the last 24 h ─────────────
        let recent_episodes = store
            .list_memories_by_layer(MemoryLayer::Episode)?
            .into_iter()
            .filter(|m| m.stored_at >= cutoff)
            .collect::<Vec<_>>();

        // Map (entity, slot) → Vec<(memory_id, confidence)> for evidence tracking.
        let mut slot_evidence: HashMap<(String, String), Vec<(String, f32)>> = HashMap::new();
        for ep in &recent_episodes {
            if !ep.entity.is_empty() && !ep.slot.is_empty() {
                slot_evidence
                    .entry((ep.entity.clone(), ep.slot.clone()))
                    .or_default()
                    .push((ep.memory_id.clone(), ep.confidence));
            }
        }

        let mut reflections = Vec::new();
        let mut reflect_candidates = Vec::new();
        for ((entity, slot), evidence) in &slot_evidence {
            let count = evidence.len();
            if count >= 3 {
                reflections.push(format!(
                    "Pattern detected: {entity}.{slot} appeared {count} times in the last 24 h"
                ));
                reflect_candidates.push(ReflectionCandidate {
                    text: format!(
                        "Pattern detected: {entity}.{slot} appeared {count} times in the last 24 h"
                    ),
                    supporting_memory_ids: evidence.iter().map(|(id, _)| id.clone()).collect(),
                    supporting_confidences: evidence.iter().map(|(_, c)| *c).collect(),
                    origin_rule: "slot-frequency".to_string(),
                });
            }
        }

        // ── Drift detection: compare belief values against recent episodes ──────
        // Gather distinct (entity, slot) pairs from the Belief layer.
        let belief_memories = store.list_memories_by_layer(MemoryLayer::Belief)?;
        let mut checked: HashSet<(String, String)> = HashSet::new();
        let mut drift_alerts = Vec::new();

        for bm in &belief_memories {
            let key = (bm.entity.clone(), bm.slot.clone());
            if bm.entity.is_empty() || bm.slot.is_empty() || !checked.insert(key) {
                continue;
            }
            if let Some(belief) = store.get_active_belief(&bm.entity, &bm.slot)? {
                let contradicting = recent_episodes
                    .iter()
                    .filter(|ep| {
                        ep.entity == belief.entity
                            && ep.slot == belief.slot
                            && !ep.value.is_empty()
                            && ep.value != belief.current_value
                    })
                    .count();

                if contradicting >= 3 {
                    drift_alerts.push(BeliefDriftAlert {
                        belief_id: belief.belief_id.clone(),
                        entity: belief.entity.clone(),
                        slot: belief.slot.clone(),
                        current_value: belief.current_value.clone(),
                        contradicting_episode_count: contradicting,
                        alert_text: format!(
                            "belief drift: {contradicting} recent episodes for {}.{} conflict with current value '{}'",
                            belief.entity, belief.slot, belief.current_value
                        ),
                    });
                }
            }
        }

        // ── Goal tracking: recurring failures ────────────────────────────────────
        let goal_memories = store.list_memories_by_layer(MemoryLayer::GoalState)?;
        let mut goal_patterns = Vec::new();
        for gm in goal_memories {
            if gm.negative_outcome_count() >= 2 {
                goal_patterns.push(GoalPattern {
                    goal_text: gm.value.clone(),
                    failure_count: gm.negative_outcome_count() as usize,
                    last_seen: gm.version_timestamp(),
                });
            }
        }

        // ── Procedure promotion: delegate to consolidation engine ─────────────────
        let outcomes = self.consolidation.consolidate(store, None, None, clock)?;
        let promoted_procedures: Vec<String> = outcomes
            .into_iter()
            .filter_map(|o| o.learned_procedure_id)
            .collect();

        Ok(ReasoningCycleResult {
            reflections,
            reflect_candidates,
            drift_alerts,
            goal_patterns,
            promoted_procedures,
            cycle_at: now,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::TimeZone;

    use super::*;
    use crate::agent_memory::adapters::memvid_store::InMemoryMemoryStore;
    use crate::agent_memory::clock::FixedClock;
    use crate::agent_memory::enums::{MemoryType, Scope, SourceType};
    use crate::agent_memory::policy::PolicySet;
    use crate::agent_memory::schemas::{DurableMemory, Provenance};

    fn fixed(ts: i64) -> FixedClock {
        FixedClock::new(chrono::Utc.timestamp_opt(ts, 0).unwrap())
    }

    fn make_episode(entity: &str, slot: &str, value: &str, stored_at_ts: i64) -> DurableMemory {
        DurableMemory {
            memory_id: uuid::Uuid::new_v4().to_string(),
            candidate_id: uuid::Uuid::new_v4().to_string(),
            stored_at: chrono::Utc.timestamp_opt(stored_at_ts, 0).unwrap(),
            updated_at: None,
            entity: entity.to_string(),
            slot: slot.to_string(),
            value: value.to_string(),
            raw_text: format!("{entity} {slot} {value}"),
            memory_type: MemoryType::Episode,
            confidence: 0.9,
            salience: 0.8,
            scope: Scope::Private,
            ttl: None,
            source: Provenance {
                source_type: SourceType::Chat,
                source_id: "test".to_string(),
                source_label: None,
                observed_by: None,
                trust_weight: 0.9,
            },
            event_at: None,
            valid_from: None,
            valid_to: None,
            internal_layer: Some(MemoryLayer::Episode),
            tags: Vec::new(),
            metadata: BTreeMap::new(),
            is_retraction: false,
        }
    }

    fn engine() -> ReasoningEngine {
        ReasoningEngine::new(ConsolidationEngine::new(PolicySet::default()))
    }

    // Each test fixes the clock at ts=1_700_000_000.
    // Memories stored at ts=1_699_990_000 (~2.7 h before) fall within the 24 h window.
    const NOW: i64 = 1_700_000_000;
    const RECENT: i64 = 1_699_990_000; // 2.7 h before NOW — within window
    const OLD: i64 = 1_699_900_000; // ~27 h before NOW — outside window

    #[test]
    fn reflection_emitted_for_high_frequency_slot() {
        let clock = fixed(NOW);
        let mut store = InMemoryMemoryStore::default();
        for _ in 0..3 {
            store
                .put_memory(&make_episode("agent", "preference", "rust", RECENT))
                .unwrap();
        }
        let result = engine().run_cycle(&mut store, &clock).unwrap();
        assert!(
            result
                .reflections
                .iter()
                .any(|r| r.contains("agent.preference")),
            "expected reflection for agent.preference, got: {:?}",
            result.reflections
        );
    }

    #[test]
    fn no_reflection_for_low_frequency_slot() {
        let clock = fixed(NOW);
        let mut store = InMemoryMemoryStore::default();
        store
            .put_memory(&make_episode("agent", "language", "rust", RECENT))
            .unwrap();
        store
            .put_memory(&make_episode("agent", "language", "python", RECENT))
            .unwrap();
        let result = engine().run_cycle(&mut store, &clock).unwrap();
        assert!(result.reflections.is_empty());
    }

    #[test]
    fn old_episodes_excluded_from_reflection() {
        let clock = fixed(NOW);
        let mut store = InMemoryMemoryStore::default();
        for _ in 0..5 {
            store
                .put_memory(&make_episode("agent", "tool", "vim", OLD))
                .unwrap();
        }
        let result = engine().run_cycle(&mut store, &clock).unwrap();
        // All episodes are outside 24 h window — no reflections
        assert!(result.reflections.is_empty());
    }

    #[test]
    fn cycle_timestamp_matches_clock() {
        let clock = fixed(NOW);
        let mut store = InMemoryMemoryStore::default();
        let result = engine().run_cycle(&mut store, &clock).unwrap();
        let expected = chrono::Utc.timestamp_opt(NOW, 0).unwrap();
        assert_eq!(result.cycle_at, expected);
    }

    #[test]
    fn empty_store_returns_empty_results() {
        let clock = fixed(NOW);
        let mut store = InMemoryMemoryStore::default();
        let result = engine().run_cycle(&mut store, &clock).unwrap();
        assert!(result.reflections.is_empty());
        assert!(result.drift_alerts.is_empty());
        assert!(result.goal_patterns.is_empty());
        assert!(result.promoted_procedures.is_empty());
    }
}
