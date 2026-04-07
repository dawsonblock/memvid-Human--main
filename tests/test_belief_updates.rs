mod common;

use std::collections::BTreeMap;

use memvid_core::agent_memory::belief_updater::BeliefUpdater;
use memvid_core::agent_memory::enums::{BeliefAction, BeliefStatus, SourceType};
use memvid_core::agent_memory::schemas::BeliefRecord;

use common::{durable, ts};

#[test]
fn first_fact_creates_belief() {
    let updater = BeliefUpdater;
    let memory = durable(
        "user",
        "location",
        "Berlin",
        "User is in Berlin",
        memvid_core::agent_memory::enums::MemoryType::Fact,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );

    let outcome = updater.apply(
        None,
        &memory,
        &memvid_core::agent_memory::clock::FixedClock::new(ts(1_700_000_000)),
    );

    assert_eq!(outcome.action, BeliefAction::Update);
    assert_eq!(
        outcome.current_belief.expect("belief").current_value,
        "Berlin"
    );
}

#[test]
fn matching_fact_reinforces_belief() {
    let updater = BeliefUpdater;
    let existing = BeliefRecord {
        belief_id: "belief-1".to_string(),
        entity: "user".to_string(),
        slot: "location".to_string(),
        current_value: "Berlin".to_string(),
        status: BeliefStatus::Active,
        confidence: 0.7,
        valid_from: ts(1_700_000_000),
        valid_to: None,
        last_reviewed_at: ts(1_700_000_000),
        supporting_memory_ids: vec!["m1".to_string()],
        opposing_memory_ids: Vec::new(),
        contradictions_observed: 0,
        last_contradiction_at: None,
        time_to_last_resolution_seconds: None,
        positive_outcome_count: 0,
        negative_outcome_count: 0,
        last_outcome_at: None,
        source_weights: BTreeMap::from([(SourceType::Chat, 0.75)]),
    };
    let memory = durable(
        "user",
        "location",
        "Berlin",
        "User is still in Berlin",
        memvid_core::agent_memory::enums::MemoryType::Fact,
        SourceType::Chat,
        0.75,
        ts(1_700_000_100),
    );

    let outcome = updater.apply(
        Some(existing),
        &memory,
        &memvid_core::agent_memory::clock::FixedClock::new(ts(1_700_000_100)),
    );

    assert_eq!(outcome.action, BeliefAction::Reinforce);
    assert!(
        outcome
            .current_belief
            .expect("belief")
            .supporting_memory_ids
            .contains(&memory.memory_id)
    );
}

#[test]
fn higher_trust_replacement_updates_belief() {
    let updater = BeliefUpdater;
    let existing = BeliefRecord {
        belief_id: "belief-1".to_string(),
        entity: "user".to_string(),
        slot: "location".to_string(),
        current_value: "Berlin".to_string(),
        status: BeliefStatus::Active,
        confidence: 0.8,
        valid_from: ts(1_700_000_000),
        valid_to: None,
        last_reviewed_at: ts(1_700_000_000),
        supporting_memory_ids: vec!["m1".to_string()],
        opposing_memory_ids: Vec::new(),
        contradictions_observed: 0,
        last_contradiction_at: None,
        time_to_last_resolution_seconds: None,
        positive_outcome_count: 0,
        negative_outcome_count: 0,
        last_outcome_at: None,
        source_weights: BTreeMap::from([(SourceType::Chat, 0.75)]),
    };
    let memory = durable(
        "user",
        "location",
        "Paris",
        "System record says the user is in Paris",
        memvid_core::agent_memory::enums::MemoryType::Fact,
        SourceType::System,
        1.0,
        ts(1_700_000_200),
    );

    let outcome = updater.apply(
        Some(existing),
        &memory,
        &memvid_core::agent_memory::clock::FixedClock::new(ts(1_700_000_200)),
    );

    assert_eq!(outcome.action, BeliefAction::Update);
    assert_eq!(
        outcome.prior_belief.expect("stale").status,
        BeliefStatus::Stale
    );
    assert_eq!(
        outcome.current_belief.expect("replacement").current_value,
        "Paris"
    );
}

#[test]
fn lower_trust_conflict_disputes_existing_belief() {
    let updater = BeliefUpdater;
    let existing = BeliefRecord {
        belief_id: "belief-1".to_string(),
        entity: "user".to_string(),
        slot: "location".to_string(),
        current_value: "Berlin".to_string(),
        status: BeliefStatus::Active,
        confidence: 0.85,
        valid_from: ts(1_700_000_000),
        valid_to: None,
        last_reviewed_at: ts(1_700_000_000),
        supporting_memory_ids: vec!["m1".to_string()],
        opposing_memory_ids: Vec::new(),
        contradictions_observed: 0,
        last_contradiction_at: None,
        time_to_last_resolution_seconds: None,
        positive_outcome_count: 0,
        negative_outcome_count: 0,
        last_outcome_at: None,
        source_weights: BTreeMap::from([(SourceType::System, 1.0)]),
    };
    let memory = durable(
        "user",
        "location",
        "Paris",
        "A tool guessed Paris",
        memvid_core::agent_memory::enums::MemoryType::Fact,
        SourceType::Tool,
        0.6,
        ts(1_700_000_200),
    );

    let outcome = updater.apply(
        Some(existing),
        &memory,
        &memvid_core::agent_memory::clock::FixedClock::new(ts(1_700_000_200)),
    );

    assert_eq!(outcome.action, BeliefAction::Dispute);
    let belief = outcome.current_belief.expect("belief");
    assert_eq!(belief.status, BeliefStatus::Disputed);
    assert_eq!(belief.current_value, "Berlin");
    assert_eq!(belief.contradictions_observed, 1);
    assert_eq!(belief.last_contradiction_at, Some(ts(1_700_000_200)));
    assert_eq!(belief.time_to_last_resolution_seconds, None);
}

#[test]
fn reinforcing_disputed_belief_restores_active_status_and_tracks_resolution_time() {
    let updater = BeliefUpdater;
    let existing = BeliefRecord {
        belief_id: "belief-1".to_string(),
        entity: "user".to_string(),
        slot: "location".to_string(),
        current_value: "Berlin".to_string(),
        status: BeliefStatus::Disputed,
        confidence: 0.85,
        valid_from: ts(1_700_000_000),
        valid_to: None,
        last_reviewed_at: ts(1_700_000_050),
        supporting_memory_ids: vec!["m1".to_string()],
        opposing_memory_ids: vec!["m2".to_string()],
        contradictions_observed: 1,
        last_contradiction_at: Some(ts(1_700_000_050)),
        time_to_last_resolution_seconds: None,
        positive_outcome_count: 0,
        negative_outcome_count: 0,
        last_outcome_at: None,
        source_weights: BTreeMap::from([(SourceType::System, 1.0)]),
    };
    let memory = durable(
        "user",
        "location",
        "Berlin",
        "A second system record confirms Berlin",
        memvid_core::agent_memory::enums::MemoryType::Fact,
        SourceType::System,
        1.0,
        ts(1_700_000_100),
    );

    let outcome = updater.apply(
        Some(existing),
        &memory,
        &memvid_core::agent_memory::clock::FixedClock::new(ts(1_700_000_100)),
    );

    assert_eq!(outcome.action, BeliefAction::Reinforce);
    let belief = outcome.current_belief.expect("belief");
    assert_eq!(belief.status, BeliefStatus::Active);
    assert_eq!(belief.current_value, "Berlin");
    assert_eq!(belief.contradictions_observed, 1);
    assert_eq!(belief.last_contradiction_at, None);
    assert_eq!(belief.time_to_last_resolution_seconds, Some(50));
}
