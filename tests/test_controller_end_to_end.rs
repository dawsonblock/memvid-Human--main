mod common;

use memvid_core::agent_memory::enums::{MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::schemas::RetrievalQuery;

use common::{apply_durable, candidate, controller, durable, ts};

#[test]
fn ingest_low_trust_fact_preserves_episode_evidence_without_promoting_truth() {
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

    assert_eq!(controller.store().memories().len(), 1);
    assert_eq!(controller.store().beliefs().len(), 0);
    assert!(!memory_id.is_empty());
    assert_eq!(
        controller.store().memories()[0].memory_layer().as_str(),
        "episode"
    );

    let promotion_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "promotion")
        .expect("promotion audit event present");
    assert_eq!(
        promotion_event
            .details
            .get("reason")
            .map(String::as_str),
        Some("belief promotion requires repeated evidence, verified source, or trusted source")
    );
    assert_eq!(
        promotion_event
            .details
            .get("fallback_layer")
            .map(String::as_str),
        Some("episode")
    );
    assert_eq!(
        promotion_event
            .details
            .get("route_basis")
            .map(String::as_str),
        Some("insufficient_evidence")
    );
}

#[test]
fn ingest_verified_fact_promotes_belief_and_audits_route() {
    let (mut controller, sink) = controller(ts(1_700_000_000));
    let mut verified = candidate(
        "user",
        "location",
        "Berlin",
        "The verified profile says the user currently lives in Berlin.",
    );
    verified
        .metadata
        .insert("verified_source".to_string(), "true".to_string());

    let memory_id = controller
        .ingest(verified)
        .expect("ingest succeeds")
        .expect("durable memory stored");

    let hits = controller
        .retrieve(RetrievalQuery {
            query_text: "what is the user's current location".to_string(),
            intent: QueryIntent::CurrentFact,
            entity: Some("user".to_string()),
            slot: Some("location".to_string()),
            scope: None,
            top_k: 3,
            as_of: None,
            include_expired: false,
        })
        .expect("retrieval succeeds");

    assert!(controller.store().memories().len() >= 2);
    assert_eq!(controller.store().beliefs().len(), 1);
    assert_eq!(hits.first().map(|hit| hit.from_belief), Some(true));
    assert_eq!(
        hits.first().and_then(|hit| hit.value.as_deref()),
        Some("Berlin")
    );
    assert!(!memory_id.is_empty());

    let events = sink.events();
    let actions: Vec<_> = events.iter().map(|event| event.action.clone()).collect();
    assert_eq!(
        actions,
        vec![
            "classification".to_string(),
            "promotion".to_string(),
            "episode_stored".to_string(),
            "memory_stored".to_string(),
            "belief_updated".to_string(),
            "retrieval".to_string(),
        ]
    );

    let promotion_event = events
        .into_iter()
        .find(|event| event.action == "promotion")
        .expect("promotion audit event present");
    assert_eq!(
        promotion_event
            .details
            .get("target_layer")
            .map(String::as_str),
        Some("belief")
    );
    assert_eq!(
        promotion_event
            .details
            .get("route_basis")
            .map(String::as_str),
        Some("verified_source")
    );
    assert_eq!(
        promotion_event
            .details
            .get("verified_source")
            .map(String::as_str),
        Some("true")
    );

    let retrieval_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "retrieval")
        .expect("retrieval audit event present");
    assert_eq!(
        retrieval_event
            .details
            .get("touched_memories")
            .map(String::as_str),
        Some("2")
    );
}

#[test]
fn retrieval_touches_returned_memories_and_persists_access_metadata() {
    let now = ts(1_700_000_100);
    let (mut controller, sink) = controller(now);
    let memory = durable(
        "user",
        "favorite_editor",
        "vim",
        "The user prefers vim for editing",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );

    let memory_id = apply_durable(&mut controller, &memory, None);

    controller
        .retrieve(RetrievalQuery {
            query_text: "what editor does the user prefer".to_string(),
            intent: QueryIntent::PreferenceLookup,
            entity: Some("user".to_string()),
            slot: None,
            scope: None,
            top_k: 1,
            as_of: None,
            include_expired: false,
        })
        .expect("retrieval succeeds");

    let latest = controller
        .store()
        .memories()
        .iter()
        .rev()
        .find(|stored| stored.memory_id == memory_id)
        .expect("touched memory present");
    assert_eq!(
        latest.metadata.get("retrieval_count").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        latest.metadata.get("last_accessed_at").map(String::as_str),
        Some(now.to_rfc3339().as_str())
    );

    let retrieval_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "retrieval")
        .expect("retrieval audit event present");
    assert_eq!(
        retrieval_event
            .details
            .get("touched_memories")
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        retrieval_event
            .details
            .get("touched_memory_ids")
            .map(String::as_str),
        Some(memory_id.as_str())
    );
}