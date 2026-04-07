mod common;

use memvid_core::agent_memory::adapters::memvid_store::InMemoryMemoryStore;
use memvid_core::agent_memory::enums::{BeliefStatus, MemoryType, SelfModelKind, SourceType};
use memvid_core::agent_memory::self_model_store::SelfModelStore;

use common::{candidate, controller, durable, ts};

#[test]
fn repeated_stable_preference_reinforces_one_logical_self_model_entry() {
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
        "The user still prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.8,
        ts(1_700_000_100),
    );

    {
        let mut self_model_store = SelfModelStore::new(&mut store);
        self_model_store
            .save_memory(&first, Some("episode-1"))
            .expect("first self-model stored");
        self_model_store
            .save_memory(&second, Some("episode-2"))
            .expect("reinforced self-model stored");
    }

    let latest = {
        let mut self_model_store = SelfModelStore::new(&mut store);
        self_model_store
            .get_latest_for_entity_slot("user", "response_style")
            .expect("latest self-model loaded")
            .expect("latest self-model exists")
    };
    let matching_records = {
        let mut self_model_store = SelfModelStore::new(&mut store);
        self_model_store
            .matching_values("user", "response_style", "concise")
            .expect("matching self-model records listed")
    };

    assert_eq!(latest.kind, SelfModelKind::ResponseStyle);
    assert_eq!(latest.status, BeliefStatus::Active);
    assert_eq!(latest.value, "concise");
    assert_eq!(latest.memory_id, first.memory_id);
    assert_eq!(
        latest.metadata.get("reinforcement_count").map(String::as_str),
        Some("2")
    );
    assert_eq!(matching_records.len(), 2);
}

#[test]
fn one_off_preference_observation_stays_episode_evidence_until_repeated() {
    let (mut controller, _) = controller(ts(1_700_000_000));

    controller
        .ingest(candidate(
            "user",
            "response_style",
            "concise",
            "The user prefers concise responses",
        ))
        .expect("ingest succeeds")
        .expect("episode evidence stored");

    let self_model_records = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .list_for_entity("user")
            .expect("self-model records listed")
    };

    assert!(self_model_records.is_empty());
    assert_eq!(controller.store().memories().len(), 1);
    assert_eq!(
        controller.store().memories()[0].memory_layer().as_str(),
        "episode"
    );
}

#[test]
fn stronger_contradictory_preference_updates_the_same_logical_trait() {
    let mut store = InMemoryMemoryStore::default();
    let first = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.7,
        ts(1_700_000_000),
    );
    let second = durable(
        "user",
        "response_style",
        "verbose",
        "The system profile now says the user prefers detailed responses",
        MemoryType::Preference,
        SourceType::System,
        1.0,
        ts(1_700_000_100),
    );

    {
        let mut self_model_store = SelfModelStore::new(&mut store);
        self_model_store
            .save_memory(&first, Some("episode-1"))
            .expect("first self-model stored");
        self_model_store
            .save_memory(&second, Some("episode-2"))
            .expect("updated self-model stored");
    }

    let latest = {
        let mut self_model_store = SelfModelStore::new(&mut store);
        self_model_store
            .get_latest_for_entity_slot("user", "response_style")
            .expect("latest self-model loaded")
            .expect("latest self-model exists")
    };

    assert_eq!(latest.value, "verbose");
    assert_eq!(latest.status, BeliefStatus::Active);
    assert_eq!(latest.memory_id, first.memory_id);
    assert_eq!(
        latest
            .metadata
            .get("contradiction_resolution")
            .map(String::as_str),
        Some("updated")
    );
}

#[test]
fn weaker_contradictory_preference_is_disputed_without_replacing_active_trait() {
    let mut store = InMemoryMemoryStore::default();
    let first = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses",
        MemoryType::Preference,
        SourceType::System,
        1.0,
        ts(1_700_000_000),
    );
    let second = durable(
        "user",
        "response_style",
        "verbose",
        "One chat turn suggested a more verbose style",
        MemoryType::Preference,
        SourceType::Chat,
        0.55,
        ts(1_700_000_100),
    );

    {
        let mut self_model_store = SelfModelStore::new(&mut store);
        self_model_store
            .save_memory(&first, Some("episode-1"))
            .expect("first self-model stored");
        self_model_store
            .save_memory(&second, Some("episode-2"))
            .expect("disputed self-model stored");
    }

    let latest = {
        let mut self_model_store = SelfModelStore::new(&mut store);
        self_model_store
            .get_latest_for_entity_slot("user", "response_style")
            .expect("latest self-model loaded")
            .expect("latest self-model exists")
    };
    let all_records = {
        let mut self_model_store = SelfModelStore::new(&mut store);
        self_model_store
            .list_for_entity("user")
            .expect("self-model records listed")
    };

    assert_eq!(latest.value, "concise");
    assert!(all_records.iter().any(|record| {
        record.value == "verbose" && record.status == BeliefStatus::Disputed
    }));
}