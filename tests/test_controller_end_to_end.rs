mod common;

use memvid_core::agent_memory::enums::QueryIntent;
use memvid_core::agent_memory::schemas::RetrievalQuery;

use common::{candidate, controller, ts};

#[test]
fn ingest_promote_update_belief_and_audit_sequence() {
    let (mut controller, sink) = controller(ts(1_700_000_000));

    let memory_id = controller
        .ingest(candidate(
            "user",
            "location",
            "Berlin",
            "The user currently lives in Berlin.",
        ))
        .expect("ingest succeeds")
        .expect("memory stored");

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

    assert_eq!(controller.store().memories().len(), 1);
    assert_eq!(controller.store().beliefs().len(), 1);
    assert_eq!(
        hits.first().and_then(|hit| hit.value.as_deref()),
        Some("Berlin")
    );
    assert_eq!(hits.first().map(|hit| hit.from_belief), Some(true));
    assert!(!memory_id.is_empty());

    let actions: Vec<_> = sink
        .events()
        .into_iter()
        .map(|event| event.action)
        .collect();
    assert_eq!(
        actions,
        vec![
            "classification".to_string(),
            "promotion".to_string(),
            "memory_stored".to_string(),
            "belief_updated".to_string(),
            "retrieval".to_string(),
        ]
    );
}
