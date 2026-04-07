mod common;

use std::collections::BTreeMap;

use memvid_core::agent_memory::belief_updater::BeliefUpdater;
use memvid_core::agent_memory::enums::{BeliefStatus, MemoryType, SourceType};
use memvid_core::agent_memory::schemas::BeliefRecord;

use common::{durable, ts};

#[test]
fn contradictory_lower_trust_memory_does_not_silently_replace_stronger_belief() {
    let updater = BeliefUpdater;
    let existing = BeliefRecord {
        belief_id: "belief-1".to_string(),
        entity: "user".to_string(),
        slot: "employer".to_string(),
        current_value: "Acme".to_string(),
        status: BeliefStatus::Active,
        confidence: 0.9,
        valid_from: ts(1_700_000_000),
        valid_to: None,
        last_reviewed_at: ts(1_700_000_000),
        supporting_memory_ids: vec!["m-strong".to_string()],
        opposing_memory_ids: Vec::new(),
        contradictions_observed: 0,
        last_contradiction_at: None,
        time_to_last_resolution_seconds: None,
        positive_outcome_count: 0,
        negative_outcome_count: 0,
        last_outcome_at: None,
        source_weights: BTreeMap::from([(SourceType::File, 0.9)]),
    };
    let conflicting = durable(
        "user",
        "employer",
        "OtherCorp",
        "tool says employer is OtherCorp",
        MemoryType::Fact,
        SourceType::Tool,
        0.6,
        ts(1_700_000_100),
    );

    let outcome = updater.apply(
        Some(existing),
        &conflicting,
        &memvid_core::agent_memory::clock::FixedClock::new(ts(1_700_000_100)),
    );
    let resulting = outcome.current_belief.expect("belief");

    assert_eq!(resulting.current_value, "Acme");
    assert_eq!(resulting.status, BeliefStatus::Disputed);
    assert_eq!(resulting.contradictions_observed, 1);
    assert_eq!(resulting.last_contradiction_at, Some(ts(1_700_000_100)));
    assert!(
        resulting
            .opposing_memory_ids
            .contains(&conflicting.memory_id)
    );
}
