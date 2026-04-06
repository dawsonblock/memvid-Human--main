mod common;

use memvid_core::agent_memory::enums::{
    BeliefStatus, GoalStatus, MemoryLayer, MemoryType, SelfModelKind, SourceType,
};

use common::{candidate, durable, ts};

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
    assert_eq!(self_model.status, BeliefStatus::Active);
    assert_eq!(self_model.value, "concise");
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
