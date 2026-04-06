mod common;

use memvid_core::agent_memory::adapters::memvid_store::{InMemoryMemoryStore, MemoryStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::consolidation_engine::ConsolidationEngine;
use memvid_core::agent_memory::enums::{MemoryLayer, MemoryType, SourceType};
use memvid_core::agent_memory::goal_state_store::GoalStateStore;
use memvid_core::agent_memory::self_model_store::SelfModelStore;

use common::{durable, ts};

#[test]
fn consolidation_records_repeated_self_model_preferences() {
    let mut store = InMemoryMemoryStore::default();
    let first = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    let second = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses during repo work",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_060),
    );

    {
        let mut self_model_store = SelfModelStore::new(&mut store);
        self_model_store
            .save_memory(&first, Some("episode-1"))
            .expect("first self-model stored");
        self_model_store
            .save_memory(&second, Some("episode-2"))
            .expect("second self-model stored");
    }

    let outcomes = ConsolidationEngine::default()
        .consolidate(
            &mut store,
            None,
            Some(&second),
            &FixedClock::new(ts(1_700_000_120)),
        )
        .expect("consolidation succeeds");

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].record.target_layer, MemoryLayer::SelfModel);
    assert!(outcomes[0].trace_id.starts_with("trace") || !outcomes[0].trace_id.is_empty());
}

#[test]
fn consolidation_records_stable_belief_windows() {
    let mut store = InMemoryMemoryStore::default();
    let mut first = durable(
        "project",
        "deployment_target",
        "staging",
        "The deployment target is staging",
        MemoryType::Fact,
        SourceType::Chat,
        0.8,
        ts(1_700_000_000),
    );
    first.internal_layer = Some(MemoryLayer::Belief);
    first.event_at = Some(ts(1_700_000_000));
    store.put_memory(&first).expect("first belief stored");

    let mut second = durable(
        "project",
        "deployment_target",
        "staging",
        "The deployment target is still staging",
        MemoryType::Fact,
        SourceType::Chat,
        0.8,
        ts(1_700_000_000 + 5 * 86_400),
    );
    second.internal_layer = Some(MemoryLayer::Belief);
    second.event_at = Some(ts(1_700_000_000 + 5 * 86_400));
    store.put_memory(&second).expect("second belief stored");

    let outcomes = ConsolidationEngine::default()
        .consolidate(
            &mut store,
            None,
            Some(&second),
            &FixedClock::new(ts(1_700_000_000 + 5 * 86_400 + 60)),
        )
        .expect("consolidation succeeds");

    assert!(
        outcomes
            .iter()
            .any(|outcome| outcome.record.target_layer == MemoryLayer::Belief)
    );
}

#[test]
fn consolidation_records_recurring_blockers() {
    let mut store = InMemoryMemoryStore::default();
    let mut first = durable(
        "project",
        "task_status",
        "blocked",
        "Blocked waiting on CI",
        MemoryType::GoalState,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    first
        .metadata
        .insert("blocker_reason".to_string(), "ci_red".to_string());

    let mut second = durable(
        "project",
        "task_status",
        "blocked",
        "Blocked waiting on CI again",
        MemoryType::GoalState,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000 + 86_400),
    );
    second
        .metadata
        .insert("blocker_reason".to_string(), "ci_red".to_string());

    let mut third = durable(
        "project",
        "task_status",
        "blocked",
        "Blocked waiting on CI for a third time",
        MemoryType::GoalState,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000 + 2 * 86_400),
    );
    third
        .metadata
        .insert("blocker_reason".to_string(), "ci_red".to_string());

    {
        let mut goal_store = GoalStateStore::new(&mut store);
        goal_store
            .save_memory(&first, None)
            .expect("first goal stored");
        goal_store
            .save_memory(&second, None)
            .expect("second goal stored");
        goal_store
            .save_memory(&third, None)
            .expect("third goal stored");
    }

    let outcomes = ConsolidationEngine::default()
        .consolidate(
            &mut store,
            None,
            Some(&third),
            &FixedClock::new(ts(1_700_000_000 + 2 * 86_400 + 60)),
        )
        .expect("consolidation succeeds");

    let blocker_outcome = outcomes
        .iter()
        .find(|outcome| outcome.record.target_layer == MemoryLayer::GoalState)
        .expect("goal-state consolidation present");
    assert_eq!(
        blocker_outcome
            .record
            .metadata
            .get("blocker_key")
            .map(String::as_str),
        Some("ci_red")
    );
}
