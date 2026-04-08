mod common;

use memvid_core::agent_memory::enums::{MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::schemas::RetrievalQuery;

use common::{apply_durable, controller, durable, ts};

#[test]
fn retrieval_query_from_text_detects_obvious_intents() {
    assert_eq!(
        RetrievalQuery::from_text("what is the user's current location").intent,
        QueryIntent::CurrentFact
    );
    assert_eq!(
        RetrievalQuery::from_text("what happened last time we deployed").intent,
        QueryIntent::EpisodicRecall
    );
    assert_eq!(
        RetrievalQuery::from_text("what was the user's location as of last week").intent,
        QueryIntent::HistoricalFact
    );
}

#[test]
fn retrieve_text_is_convenience_over_typed_retrieval() {
    let (mut controller, _) = controller(ts(1_700_000_100));
    let memory = durable(
        "user",
        "favorite_editor",
        "vim",
        "The user prefers vim for editing",
        MemoryType::Preference,
        SourceType::Chat,
        0.8,
        ts(1_700_000_000),
    );
    apply_durable(&mut controller, &memory, None);

    let convenience = controller
        .retrieve_text("what editor does the user prefer")
        .expect("convenience retrieval succeeds");
    let typed = controller
        .retrieve(RetrievalQuery {
            query_text: "what editor does the user prefer".to_string(),
            intent: QueryIntent::PreferenceLookup,
            entity: Some("user".to_string()),
            slot: None,
            scope: None,
            top_k: 5,
            as_of: None,
            include_expired: false,
        })
        .expect("typed retrieval succeeds");

    assert_eq!(
        convenience.first().and_then(|hit| hit.value.as_deref()),
        typed.first().and_then(|hit| hit.value.as_deref())
    );
    assert_eq!(
        RetrievalQuery::from_text("what editor does the user prefer").intent,
        QueryIntent::PreferenceLookup
    );
}
