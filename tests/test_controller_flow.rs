mod common;

use memvid_core::agent_memory::enums::{MemoryLayer, SourceType};
use memvid_core::agent_memory::self_model_store::SelfModelStore;

use common::{candidate, controller, ts};

#[test]
fn low_trust_fact_routes_to_episode_evidence_and_audits_why() {
    let (mut controller, sink) = controller(ts(1_700_000_000));

    let memory_id = controller
        .ingest(candidate(
            "user",
            "location",
            "Berlin",
            "The user currently lives in Berlin.",
        ))
        .expect("ingest succeeds")
        .expect("episode evidence stored");

    assert!(!memory_id.is_empty());
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

    let promotion_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "promotion")
        .expect("promotion event present");
    assert_eq!(
        promotion_event.details.get("target_layer").map(String::as_str),
        Some("belief")
    );
    assert_eq!(
        promotion_event.details.get("route_basis").map(String::as_str),
        Some("insufficient_evidence")
    );
    assert_eq!(
        promotion_event.details.get("fallback_layer").map(String::as_str),
        Some("episode")
    );
}

#[test]
fn explicit_trusted_preference_routes_to_self_model_and_audits_basis() {
    let (mut controller, sink) = controller(ts(1_700_000_000));
    let mut trusted = candidate(
        "user",
        "favorite_editor",
        "vim",
        "The tool profile says the user always prefers vim for editing.",
    );
    trusted.source.source_type = SourceType::Tool;
    trusted.source.source_id = "tool-profile".to_string();
    trusted.source.trust_weight = 0.95;

    let memory_id = controller
        .ingest(trusted)
        .expect("ingest succeeds")
        .expect("self-model stored");

    let self_model = {
        let mut self_model_store = SelfModelStore::new(controller.store_mut());
        self_model_store
            .get_latest_for_entity_slot("user", "favorite_editor")
            .expect("self-model lookup succeeds")
            .expect("self-model exists")
    };

    assert_eq!(memory_id, self_model.memory_id);
    assert_eq!(self_model.value, "vim");
    assert!(controller
        .store()
        .memories()
        .iter()
        .any(|memory| memory.memory_layer() == MemoryLayer::Episode));
    assert!(controller
        .store()
        .memories()
        .iter()
        .any(|memory| memory.memory_layer() == MemoryLayer::SelfModel));

    let promotion_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "promotion")
        .expect("promotion event present");
    assert_eq!(
        promotion_event.details.get("target_layer").map(String::as_str),
        Some("self_model")
    );
    assert_eq!(
        promotion_event.details.get("route_basis").map(String::as_str),
        Some("trusted_source")
    );
    assert_eq!(
        promotion_event.details.get("fallback_layer").map(String::as_str),
        Some("episode")
    );
}

#[test]
fn untrusted_preference_routes_to_episode_evidence_and_audits_why() {
    let (mut controller, sink) = controller(ts(1_700_000_000));

    let memory_id = controller
        .ingest(candidate(
            "user",
            "favorite_editor",
            "vim",
            "The user prefers vim for editing.",
        ))
        .expect("ingest succeeds")
        .expect("episode evidence stored");

    assert!(!memory_id.is_empty());
    assert!(!controller
        .store()
        .memories()
        .iter()
        .any(|memory| memory.memory_layer() == MemoryLayer::SelfModel));
    assert_eq!(
        controller
            .store()
            .memories()
            .iter()
            .filter(|memory| memory.memory_layer() == MemoryLayer::Episode)
            .count(),
        1
    );

    let promotion_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "promotion")
        .expect("promotion event present");
    assert_eq!(
        promotion_event.details.get("target_layer").map(String::as_str),
        Some("self_model")
    );
    assert_eq!(
        promotion_event.details.get("route_basis").map(String::as_str),
        Some("insufficient_evidence")
    );
    assert_eq!(
        promotion_event.details.get("fallback_layer").map(String::as_str),
        Some("episode")
    );
}

#[test]
fn unseeded_procedure_routes_to_episode_evidence_and_audits_why() {
    let (mut controller, sink) = controller(ts(1_700_000_000));
    let mut candidate = candidate(
        "procedure",
        "repo_review",
        "repo_review",
        "Review the repo in a consistent order.",
    );
    candidate.source.source_type = SourceType::Tool;
    candidate.source.source_id = "tool-seed".to_string();
    candidate.source.trust_weight = 0.95;
    candidate.internal_layer = Some(MemoryLayer::Procedure);
    candidate
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());

    let memory_id = controller
        .ingest(candidate)
        .expect("ingest succeeds")
        .expect("episode evidence stored");

    assert!(!memory_id.is_empty());
    assert!(!controller
        .store()
        .memories()
        .iter()
        .any(|memory| memory.memory_layer() == MemoryLayer::Procedure));
    assert_eq!(
        controller
            .store()
            .memories()
            .iter()
            .filter(|memory| memory.memory_layer() == MemoryLayer::Episode)
            .count(),
        1
    );

    let promotion_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "promotion")
        .expect("promotion event present");
    assert_eq!(
        promotion_event.details.get("target_layer").map(String::as_str),
        Some("procedure")
    );
    assert_eq!(
        promotion_event.details.get("route_basis").map(String::as_str),
        Some("insufficient_evidence")
    );
    assert_eq!(
        promotion_event.details.get("fallback_layer").map(String::as_str),
        Some("episode")
    );
}

#[test]
fn whitespace_only_belief_structure_falls_back_to_trace_and_never_persists_truth() {
    let (mut controller, sink) = controller(ts(1_700_000_000));
    let mut malformed = candidate(
        "user",
        "location",
        "Berlin",
        "The user currently lives in Berlin.",
    );
    malformed.entity = Some("   ".to_string());
    malformed.slot = Some("   ".to_string());
    malformed.value = Some("   ".to_string());
    malformed.internal_layer = Some(MemoryLayer::Belief);

    let trace_id = controller
        .ingest(malformed)
        .expect("ingest succeeds")
        .expect("trace stored");

    assert!(!trace_id.is_empty());
    assert!(controller.store().beliefs().is_empty());
    assert!(controller.store().memories().is_empty());
    assert_eq!(controller.store().traces().len(), 1);

    let promotion_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "promotion")
        .expect("promotion event present");
    assert_eq!(
        promotion_event.details.get("target_layer").map(String::as_str),
        Some("belief")
    );
    assert_eq!(
        promotion_event.details.get("route_basis").map(String::as_str),
        Some("insufficient_structure")
    );
    assert_eq!(
        promotion_event.details.get("fallback_layer").map(String::as_str),
        Some("trace")
    );
}