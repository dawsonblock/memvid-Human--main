mod common;

use memvid_core::agent_memory::adapters::memvid_store::{InMemoryMemoryStore, MemoryStore};
use memvid_core::agent_memory::enums::{
    BeliefStatus, GoalStatus, MemoryLayer, MemoryType, ProcedureStatus, SelfModelKind,
    SelfModelStabilityClass, SelfModelUpdateRequirement, SourceType,
};
use memvid_core::agent_memory::goal_state_store::GoalStateStore;
use memvid_core::agent_memory::policy::{PolicyProfile, PolicySet};
use memvid_core::agent_memory::schemas::{CandidateMemory, Provenance};

use common::{candidate, controller, durable, ts};

#[test]
fn public_memory_types_map_to_internal_layers() {
    let fact = durable(
        "user",
        "location",
        "Berlin",
        "The user currently lives in Berlin",
        MemoryType::Fact,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    let preference = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    let goal = durable(
        "project",
        "task_status",
        "blocked",
        "The current task is blocked waiting on review",
        MemoryType::GoalState,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );

    assert_eq!(fact.memory_layer(), MemoryLayer::Belief);
    assert_eq!(preference.memory_layer(), MemoryLayer::SelfModel);
    assert_eq!(goal.memory_layer(), MemoryLayer::GoalState);
    assert_eq!(MemoryType::Episode.memory_layer(), MemoryLayer::Episode);
    assert_eq!(MemoryType::Trace.memory_layer(), MemoryLayer::Trace);
}

#[test]
fn policy_profile_wraps_current_policy_defaults_without_changing_thresholds() {
    let policy = PolicySet::default();
    let profile = policy.policy_profile();

    assert_eq!(profile.version(), 1);
    assert_eq!(profile.reject_threshold(), policy.reject_threshold());
    assert_eq!(
        profile.store_trace_threshold(),
        policy.store_trace_threshold()
    );
    assert_eq!(
        profile.promote_threshold(MemoryLayer::Belief),
        policy.promote_threshold(MemoryLayer::Belief)
    );
    assert_eq!(
        profile.promote_threshold(MemoryLayer::SelfModel),
        policy.promote_threshold(MemoryLayer::SelfModel)
    );
    assert!(
        profile
            .hard_constraints()
            .require_non_empty_structured_identity
    );
    assert!(profile.hard_constraints().protect_self_model_identity);
    assert!(profile.soft_weights().content_match > 0.0);
    assert_eq!(profile, PolicyProfile::default());
}

#[test]
fn candidate_memory_reports_internal_layer_and_event_timestamp() {
    let mut input = candidate(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses",
    );
    input.memory_type = MemoryType::Preference;
    input.event_at = Some(ts(1_700_000_025));

    assert_eq!(input.memory_layer(), MemoryLayer::SelfModel);
    assert_eq!(input.event_timestamp(), ts(1_700_000_025));
}

#[test]
fn durable_memory_projects_goal_and_self_model_records() {
    let mut goal_memory = durable(
        "project",
        "task_status",
        "blocked",
        "The current task is blocked waiting on review",
        MemoryType::GoalState,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    goal_memory.ttl = Some(600);

    let goal = goal_memory.to_goal_record().expect("goal record");
    assert_eq!(goal.status, GoalStatus::Blocked);
    assert_eq!(goal.expires_at, Some(ts(1_700_000_600)));

    let preference_memory = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    let self_model = preference_memory
        .to_self_model_record()
        .expect("self model record");

    assert_eq!(self_model.kind, SelfModelKind::ResponseStyle);
    assert_eq!(
        self_model.stability_class,
        SelfModelStabilityClass::FlexiblePreference
    );
    assert_eq!(
        self_model.update_requirement,
        SelfModelUpdateRequirement::ReinforcementAllowed
    );
    assert_eq!(self_model.status, BeliefStatus::Active);
    assert_eq!(self_model.value, "concise");
}

#[test]
fn stable_directive_projection_defaults_from_self_model_kind() {
    let directive_memory = durable(
        "agent",
        "memory_constraint",
        "preserve_traceability",
        "Preserve traceability for durable memory changes.",
        MemoryType::Preference,
        SourceType::System,
        1.0,
        ts(1_700_000_000),
    );

    let directive = directive_memory
        .to_self_model_record()
        .expect("self-model directive record");

    assert_eq!(directive.kind, SelfModelKind::Constraint);
    assert_eq!(
        directive.stability_class,
        SelfModelStabilityClass::StableDirective
    );
    assert_eq!(
        directive.update_requirement,
        SelfModelUpdateRequirement::TrustedOrCorroborated
    );
}

#[test]
fn dangerous_layer_projections_fail_closed_on_blank_structure() {
    let mut goal_memory = durable(
        "project",
        "task_status",
        "blocked",
        "The current task is blocked waiting on review",
        MemoryType::GoalState,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    goal_memory.entity = "   ".to_string();
    assert!(goal_memory.to_goal_record().is_none());

    let mut preference_memory = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    preference_memory.value = "   ".to_string();
    assert!(preference_memory.to_self_model_record().is_none());

    let mut procedure_memory = durable(
        "procedure",
        "repo_review",
        "repo_review",
        "Start with blockers, then validate runtime surface, then check tests.",
        MemoryType::Trace,
        SourceType::System,
        1.0,
        ts(1_700_000_000),
    );
    procedure_memory.internal_layer = Some(MemoryLayer::Procedure);
    procedure_memory
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    procedure_memory.value = "   ".to_string();
    assert!(procedure_memory.to_procedure_record().is_none());
}

#[test]
fn episode_projection_preserves_time_source_and_outcome() {
    let mut episode_memory = durable(
        "project",
        "event",
        "review_requested",
        "The team requested review for the current task",
        MemoryType::Episode,
        SourceType::Tool,
        0.6,
        ts(1_700_000_000),
    );
    episode_memory.event_at = Some(ts(1_700_000_010));
    episode_memory
        .metadata
        .insert("outcome".to_string(), "pending".to_string());

    let episode = episode_memory.to_episode_record();

    assert_eq!(episode.event_at, ts(1_700_000_010));
    assert_eq!(episode.outcome.as_deref(), Some("pending"));
    assert_eq!(episode.source.source_type, SourceType::Tool);
}

#[test]
fn procedure_projection_preserves_workflow_metadata() {
    let mut procedure_memory = durable(
        "procedure",
        "repo_review",
        "repo_review",
        "Start with blockers, then validate runtime surface, then check tests.",
        MemoryType::Trace,
        SourceType::System,
        1.0,
        ts(1_700_000_000),
    );
    procedure_memory.internal_layer = Some(MemoryLayer::Procedure);
    procedure_memory
        .metadata
        .insert("procedure_name".to_string(), "repo_review".to_string());
    procedure_memory
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    procedure_memory
        .metadata
        .insert("success_count".to_string(), "2".to_string());
    procedure_memory
        .metadata
        .insert("failure_count".to_string(), "0".to_string());
    procedure_memory.metadata.insert(
        "procedure_status".to_string(),
        ProcedureStatus::Active.as_str().to_string(),
    );

    let procedure = procedure_memory
        .to_procedure_record()
        .expect("procedure record");

    assert_eq!(procedure.name, "repo_review");
    assert_eq!(procedure.success_count, 2);
    assert_eq!(procedure.status, ProcedureStatus::Active);
}

#[test]
fn low_trust_fact_is_preserved_as_episode_not_current_truth() {
    let (mut controller, _) = controller(ts(1_700_000_000));

    controller
        .ingest(candidate(
            "user",
            "location",
            "Berlin",
            "The user currently lives in Berlin",
        ))
        .expect("ingest succeeds")
        .expect("episode stored");

    assert_eq!(controller.store().beliefs().len(), 0);
    assert_eq!(controller.store().memories().len(), 1);
    assert_eq!(
        controller.store().memories()[0].memory_layer(),
        MemoryLayer::Episode
    );
}

#[test]
fn high_trust_chat_fact_still_requires_repetition_for_belief() {
    let (mut controller, _) = controller(ts(1_700_000_000));
    let mut fact = candidate(
        "user",
        "location",
        "Berlin",
        "The user currently lives in Berlin",
    );
    fact.source.trust_weight = 0.95;

    controller
        .ingest(fact)
        .expect("ingest succeeds")
        .expect("episode stored");

    assert_eq!(controller.store().beliefs().len(), 0);
    assert_eq!(
        controller
            .store()
            .memories()
            .iter()
            .filter(|memory| memory.memory_layer() == MemoryLayer::Episode)
            .count(),
        1
    );
}

#[test]
fn clear_goal_state_promotes_more_easily_than_self_model_or_belief() {
    let (mut controller, _) = controller(ts(1_700_000_000));

    let goal_id = controller
        .ingest(candidate(
            "project",
            "task_status",
            "blocked",
            "The current task is blocked waiting on review",
        ))
        .expect("goal ingest succeeds")
        .expect("goal stored");
    let self_model_id = controller
        .ingest(candidate(
            "user",
            "response_style",
            "concise",
            "The user prefers concise responses",
        ))
        .expect("self-model ingest succeeds")
        .expect("episode stored");

    assert!(!goal_id.is_empty());
    assert!(!self_model_id.is_empty());
    assert!(
        controller
            .store()
            .memories()
            .iter()
            .any(|memory| memory.memory_layer() == MemoryLayer::GoalState)
    );
    assert!(
        controller
            .store()
            .memories()
            .iter()
            .any(|memory| memory.memory_layer() == MemoryLayer::Episode)
    );
    assert!(
        !controller
            .store()
            .memories()
            .iter()
            .any(|memory| memory.memory_layer() == MemoryLayer::SelfModel)
    );
}

#[test]
fn newer_invalid_goal_state_row_does_not_hide_older_valid_active_goal() {
    let mut store = InMemoryMemoryStore::default();
    let valid_goal = durable(
        "project",
        "task_status",
        "blocked",
        "The current task is blocked waiting on review",
        MemoryType::GoalState,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    store.put_memory(&valid_goal).expect("valid goal stored");

    let mut invalid_goal = valid_goal.clone();
    invalid_goal.memory_id = "goal-invalid".to_string();
    invalid_goal.stored_at = ts(1_700_000_100);
    invalid_goal.value = "   ".to_string();
    store
        .put_memory(&invalid_goal)
        .expect("invalid goal stored");

    let active_goals = {
        let mut goal_store = GoalStateStore::new(&mut store);
        goal_store.list_active().expect("active goals listed")
    };

    assert_eq!(active_goals.len(), 1);
    assert_eq!(active_goals[0].memory_id, valid_goal.memory_id);
    assert_eq!(active_goals[0].value, "blocked");
}

#[test]
fn procedure_requires_system_seed_or_repeated_evidence() {
    let (mut controller, _) = controller(ts(1_700_000_000));
    let unseeded = CandidateMemory {
        candidate_id: "unseeded-procedure".to_string(),
        observed_at: ts(1_700_000_000),
        entity: Some("procedure".to_string()),
        slot: Some("repo_review".to_string()),
        value: Some("repo_review".to_string()),
        raw_text: "Review the repo in a consistent order".to_string(),
        source: Provenance {
            source_type: SourceType::Tool,
            source_id: "tool-seed".to_string(),
            source_label: Some("tool".to_string()),
            observed_by: None,
            trust_weight: 0.9,
        },
        memory_type: MemoryType::Trace,
        confidence: 0.95,
        salience: 0.9,
        scope: memvid_core::agent_memory::enums::Scope::Project,
        ttl: None,
        event_at: None,
        valid_from: None,
        valid_to: None,
        internal_layer: Some(MemoryLayer::Procedure),
        tags: vec!["review".to_string()],
        metadata: std::collections::BTreeMap::from([(
            "workflow_key".to_string(),
            "repo_review".to_string(),
        )]),
        is_retraction: false,
    };
    controller
        .ingest(unseeded)
        .expect("unseeded ingest succeeds")
        .expect("evidence stored");
    assert!(
        !controller
            .store()
            .memories()
            .iter()
            .any(|memory| memory.memory_layer() == MemoryLayer::Procedure)
    );

    let seeded = CandidateMemory {
        candidate_id: "seeded-procedure".to_string(),
        observed_at: ts(1_700_000_100),
        entity: Some("procedure".to_string()),
        slot: Some("repo_review".to_string()),
        value: Some("repo_review".to_string()),
        raw_text: "Review the repo in a consistent order".to_string(),
        source: Provenance {
            source_type: SourceType::System,
            source_id: "system-seed".to_string(),
            source_label: Some("system".to_string()),
            observed_by: None,
            trust_weight: 1.0,
        },
        memory_type: MemoryType::Trace,
        confidence: 0.95,
        salience: 0.9,
        scope: memvid_core::agent_memory::enums::Scope::Project,
        ttl: None,
        event_at: None,
        valid_from: None,
        valid_to: None,
        internal_layer: Some(MemoryLayer::Procedure),
        tags: vec!["review".to_string()],
        metadata: std::collections::BTreeMap::from([
            ("workflow_key".to_string(), "repo_review".to_string()),
            ("seeded_by_system".to_string(), "true".to_string()),
        ]),
        is_retraction: false,
    };
    controller
        .ingest(seeded)
        .expect("seeded ingest succeeds")
        .expect("procedure stored");

    assert!(
        controller
            .store()
            .memories()
            .iter()
            .any(|memory| memory.memory_layer() == MemoryLayer::Procedure)
    );
}
