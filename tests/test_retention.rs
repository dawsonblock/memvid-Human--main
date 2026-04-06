mod common;

use memvid_core::agent_memory::enums::{MemoryType, SourceType};
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::retention::RetentionManager;

use common::{durable, ts};

#[test]
fn trace_memory_expires_by_ttl() {
    let retention = RetentionManager::new(PolicySet::default());
    let mut memory = durable(
        "user",
        "note",
        "temp",
        "temporary trace",
        MemoryType::Trace,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    memory.ttl = Some(60);

    let evaluation = retention.evaluate(&memory, ts(1_700_000_061));

    assert!(evaluation.expired);
}

#[test]
fn durable_preference_does_not_expire_by_default() {
    let retention = RetentionManager::new(PolicySet::default());
    let memory = durable(
        "user",
        "favorite_color",
        "blue",
        "prefers blue",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );

    let evaluation = retention.evaluate(&memory, ts(1_900_000_000));

    assert!(!evaluation.expired);
}

#[test]
fn decay_score_drops_with_age() {
    let retention = RetentionManager::new(PolicySet::default());
    let memory = durable(
        "project",
        "task_status",
        "open",
        "task remains open",
        MemoryType::GoalState,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );

    let fresh = retention.evaluate(&memory, ts(1_700_000_000));
    let aged = retention.evaluate(&memory, ts(1_700_000_000 + 20 * 86_400));

    assert!(aged.decayed_salience < fresh.decayed_salience);
}
