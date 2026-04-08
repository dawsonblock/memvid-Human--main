mod common;

use memvid_core::agent_memory::enums::{MemoryLayer, MemoryType, ProcedureStatus, SourceType};
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

#[test]
fn procedure_lifecycle_changes_retention_and_expiry() {
    let retention = RetentionManager::new(PolicySet::default());
    let active = procedure_memory(
        ProcedureStatus::Active,
        8,
        1,
        ts(1_700_000_000),
        Some(ts(1_700_000_000)),
    );
    let cooling = procedure_memory(
        ProcedureStatus::CoolingDown,
        4,
        5,
        ts(1_700_000_000),
        Some(ts(1_700_000_000)),
    );
    let retired = procedure_memory(
        ProcedureStatus::Retired,
        1,
        7,
        ts(1_700_000_000),
        Some(ts(1_700_000_000)),
    );

    let active_eval = retention.evaluate(&active, ts(1_700_000_000 + 20 * 86_400));
    let cooling_eval = retention.evaluate(&cooling, ts(1_700_000_000 + 20 * 86_400));
    let retired_eval = retention.evaluate(&retired, ts(1_700_000_000 + 20 * 86_400));

    assert!(!active_eval.expired);
    assert!(!cooling_eval.expired);
    assert!(retired_eval.expired);
    assert!(active_eval.decayed_salience > cooling_eval.decayed_salience);
    assert!(cooling_eval.decayed_salience > retired_eval.decayed_salience);
}

#[test]
fn failure_heavy_procedures_decay_faster_even_when_marked_active() {
    let retention = RetentionManager::new(PolicySet::default());
    let successful = procedure_memory(
        ProcedureStatus::Active,
        6,
        1,
        ts(1_700_000_000),
        Some(ts(1_700_000_000)),
    );
    let failure_heavy = procedure_memory(
        ProcedureStatus::Active,
        1,
        6,
        ts(1_700_000_000),
        Some(ts(1_700_000_000)),
    );

    let successful_eval = retention.evaluate(&successful, ts(1_700_000_000 + 30 * 86_400));
    let failure_eval = retention.evaluate(&failure_heavy, ts(1_700_000_000 + 30 * 86_400));

    assert!(successful_eval.decayed_salience > failure_eval.decayed_salience);
}

#[test]
fn recently_accessed_memory_gets_a_salience_boost() {
    let retention = RetentionManager::new(PolicySet::default());
    let baseline = durable(
        "project",
        "note",
        "build",
        "Rust build checklist",
        MemoryType::Episode,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    let mut accessed = baseline.clone();
    accessed
        .metadata
        .insert("retrieval_count".to_string(), "4".to_string());
    accessed.metadata.insert(
        "last_accessed_at".to_string(),
        ts(1_700_000_000 + 86_400).to_rfc3339(),
    );

    let baseline_eval = retention.evaluate(&baseline, ts(1_700_000_000 + 2 * 86_400));
    let accessed_eval = retention.evaluate(&accessed, ts(1_700_000_000 + 2 * 86_400));

    assert_eq!(baseline_eval.expired, accessed_eval.expired);
    assert!(accessed_eval.access_boost > 0.0);
    assert!(accessed_eval.decayed_salience > baseline_eval.decayed_salience);
}

#[test]
fn retrieval_touch_does_not_reset_ttl_anchor() {
    let retention = RetentionManager::new(PolicySet::default());
    let mut memory = durable(
        "project",
        "note",
        "temp",
        "Temporary note",
        MemoryType::Trace,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    memory.ttl = Some(60);
    memory = memory.with_retrieval_access(ts(1_700_000_030));

    let evaluation = retention.evaluate(&memory, ts(1_700_000_061));

    assert!(evaluation.expired);
    assert_eq!(memory.stored_at, ts(1_700_000_000));
    assert_eq!(memory.version_timestamp(), ts(1_700_000_030));
}

#[test]
fn positive_outcome_feedback_outranks_negative_feedback_in_salience() {
    let retention = RetentionManager::new(PolicySet::default());
    let mut positive = durable(
        "project",
        "note",
        "review",
        "Review checklist",
        MemoryType::Episode,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    positive
        .metadata
        .insert("positive_outcome_count".to_string(), "3".to_string());
    positive.metadata.insert(
        "last_outcome_at".to_string(),
        ts(1_700_000_000 + 86_400).to_rfc3339(),
    );
    let mut negative = positive.clone();
    negative.metadata.remove("positive_outcome_count");
    negative
        .metadata
        .insert("negative_outcome_count".to_string(), "3".to_string());

    let positive_eval = retention.evaluate(&positive, ts(1_700_000_000 + 2 * 86_400));
    let negative_eval = retention.evaluate(&negative, ts(1_700_000_000 + 2 * 86_400));

    assert!(positive_eval.outcome_impact_adjustment > 0.0);
    assert!(negative_eval.outcome_impact_adjustment < 0.0);
    assert!(positive_eval.decayed_salience > negative_eval.decayed_salience);
}

fn procedure_memory(
    status: ProcedureStatus,
    success_count: u32,
    failure_count: u32,
    stored_at: chrono::DateTime<chrono::Utc>,
    last_used_at: Option<chrono::DateTime<chrono::Utc>>,
) -> memvid_core::agent_memory::schemas::DurableMemory {
    let mut memory = durable(
        "procedure",
        "repo_review",
        "repo_review",
        "Review the repo in a consistent order",
        MemoryType::Trace,
        SourceType::System,
        1.0,
        stored_at,
    );
    memory.internal_layer = Some(MemoryLayer::Procedure);
    memory
        .metadata
        .insert("procedure_name".to_string(), "repo_review".to_string());
    memory
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    memory
        .metadata
        .insert("context_tags".to_string(), "repo_review,review".to_string());
    memory
        .metadata
        .insert("success_count".to_string(), success_count.to_string());
    memory
        .metadata
        .insert("failure_count".to_string(), failure_count.to_string());
    memory
        .metadata
        .insert("procedure_status".to_string(), status.as_str().to_string());
    if let Some(last_used_at) = last_used_at {
        memory
            .metadata
            .insert("last_used_at".to_string(), last_used_at.to_rfc3339());
    }
    memory
}
