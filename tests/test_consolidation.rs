mod common;

use memvid_core::agent_memory::adapters::memvid_store::{InMemoryMemoryStore, MemoryStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::consolidation_engine::ConsolidationEngine;
use memvid_core::agent_memory::enums::{MemoryLayer, MemoryType, SourceType};

use common::{apply_durable, controller, durable, ts};

#[test]
fn consolidation_records_repeated_self_model_preferences() {
    let (mut controller, _) = controller(ts(1_700_000_120));
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

    apply_durable(&mut controller, &first, Some("episode-1"));
    apply_durable(&mut controller, &second, Some("episode-2"));

    let outcomes = ConsolidationEngine::default()
        .consolidate(
            controller.store_mut(),
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
    let (mut controller, _) = controller(ts(1_700_000_000 + 2 * 86_400 + 60));
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

    apply_durable(&mut controller, &first, None);
    apply_durable(&mut controller, &second, None);
    apply_durable(&mut controller, &third, None);

    let outcomes = ConsolidationEngine::default()
        .consolidate(
            controller.store_mut(),
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

#[test]
fn self_model_consolidation_is_threshold_based_idempotent_and_reinforcing() {
    let (mut controller, _) = controller(ts(1_700_000_240));
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
        "The user still prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_060),
    );
    let third = durable(
        "user",
        "response_style",
        "concise",
        "The user again prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.8,
        ts(1_700_000_120),
    );

    apply_durable(&mut controller, &first, Some("episode-1"));
    apply_durable(&mut controller, &second, Some("episode-2"));

    let first_outcomes = ConsolidationEngine::default()
        .consolidate(
            controller.store_mut(),
            None,
            Some(&second),
            &FixedClock::new(ts(1_700_000_180)),
        )
        .expect("consolidation succeeds");
    assert_eq!(
        first_outcomes[0]
            .record
            .metadata
            .get("consolidation_action")
            .map(String::as_str),
        Some("promotion")
    );

    let rerun_outcomes = ConsolidationEngine::default()
        .consolidate(
            controller.store_mut(),
            None,
            Some(&second),
            &FixedClock::new(ts(1_700_000_181)),
        )
        .expect("rerun consolidation succeeds");
    assert!(rerun_outcomes.is_empty());

    apply_durable(&mut controller, &third, Some("episode-3"));

    let reinforcement_outcomes = ConsolidationEngine::default()
        .consolidate(
            controller.store_mut(),
            None,
            Some(&third),
            &FixedClock::new(ts(1_700_000_240)),
        )
        .expect("reinforcement consolidation succeeds");
    let reinforcement = reinforcement_outcomes
        .iter()
        .find(|outcome| outcome.record.target_layer == MemoryLayer::SelfModel)
        .expect("self-model consolidation present");
    assert_eq!(
        reinforcement
            .record
            .metadata
            .get("consolidation_action")
            .map(String::as_str),
        Some("reinforcement")
    );
}

#[test]
fn blocker_consolidation_uses_at_least_threshold_not_exact_count() {
    let (mut controller, _) = controller(ts(1_700_000_000 + (4 * 86_400)));
    for offset in 0..4 {
        let mut goal = durable(
            "project",
            "task_status",
            "blocked",
            "Blocked waiting on CI",
            MemoryType::GoalState,
            SourceType::Chat,
            0.75,
            ts(1_700_000_000 + (offset * 86_400)),
        );
        goal.metadata
            .insert("blocker_reason".to_string(), "ci_red".to_string());
        apply_durable(&mut controller, &goal, None);
    }

    let latest = controller
        .store_mut()
        .list_memories_by_layer(MemoryLayer::GoalState)
        .expect("goals listed")
        .into_iter()
        .max_by(|left, right| left.stored_at.cmp(&right.stored_at))
        .expect("latest goal exists");
    let outcomes = ConsolidationEngine::default()
        .consolidate(
            controller.store_mut(),
            None,
            Some(&latest),
            &FixedClock::new(ts(1_700_000_000 + (4 * 86_400))),
        )
        .expect("consolidation succeeds");

    let blocker = outcomes
        .iter()
        .find(|outcome| outcome.record.target_layer == MemoryLayer::GoalState)
        .expect("goal-state consolidation present");
    assert_eq!(
        blocker.record.metadata.get("threshold").map(String::as_str),
        Some("3")
    );
    assert_eq!(
        blocker
            .record
            .metadata
            .get("consolidation_action")
            .map(String::as_str),
        Some("promotion")
    );
}
