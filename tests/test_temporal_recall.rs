mod common;

use memvid_core::agent_memory::adapters::memvid_store::{InMemoryMemoryStore, MemoryStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::{MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::memory_retriever::MemoryRetriever;
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::ranker::Ranker;
use memvid_core::agent_memory::retention::RetentionManager;
use memvid_core::agent_memory::schemas::RetrievalQuery;

use common::{apply_durable, controller, durable, ts};

#[test]
fn historical_query_as_of_time_returns_past_value_rather_than_current_belief() {
    let mut store = InMemoryMemoryStore::default();
    let older = durable(
        "user",
        "location",
        "Berlin",
        "User lived in Berlin",
        MemoryType::Fact,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    let newer = durable(
        "user",
        "location",
        "Paris",
        "User moved to Paris",
        MemoryType::Fact,
        SourceType::File,
        0.9,
        ts(1_700_100_000),
    );
    store.put_memory(&older).expect("older stored");
    store.put_memory(&newer).expect("newer stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "where was the user as of last month".to_string(),
                intent: QueryIntent::HistoricalFact,
                entity: Some("user".to_string()),
                slot: Some("location".to_string()),
                scope: None,
                top_k: 3,
                as_of: Some(ts(1_700_050_000)),
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_200_000)),
        )
        .expect("retrieval works");

    assert_eq!(
        hits.first().and_then(|hit| hit.value.as_deref()),
        Some("Berlin")
    );
}

#[test]
fn retrieval_touch_does_not_move_historical_visibility_window() {
    let ingested_at = ts(1_700_000_000);
    let accessed_at = ts(1_700_100_000);
    let (mut controller, _) = controller(accessed_at);
    let memory = durable(
        "user",
        "timezone",
        "UTC+1",
        "The user usually works in UTC+1",
        MemoryType::Fact,
        SourceType::Chat,
        0.8,
        ingested_at,
    );
    let memory_id = apply_durable(&mut controller, &memory, None);

    controller
        .retrieve(RetrievalQuery {
            query_text: "what timezone does the user use".to_string(),
            intent: QueryIntent::CurrentFact,
            entity: Some("user".to_string()),
            slot: Some("timezone".to_string()),
            scope: None,
            top_k: 1,
            as_of: None,
            include_expired: false,
        })
        .expect("retrieval succeeds");

    let stored = controller
        .store()
        .memories()
        .iter()
        .find(|memory| memory.memory_id == memory_id)
        .expect("memory stored")
        .clone();
    let effective = controller
        .store_mut()
        .get_memory(&memory_id)
        .expect("lookup succeeds")
        .expect("memory available");
    assert_eq!(stored.stored_at, ingested_at);
    assert_eq!(stored.version_timestamp(), ingested_at);
    assert_eq!(effective.version_timestamp(), accessed_at);

    let historical_hits = controller
        .retrieve(RetrievalQuery {
            query_text: "what timezone did the user use as of earlier".to_string(),
            intent: QueryIntent::HistoricalFact,
            entity: Some("user".to_string()),
            slot: Some("timezone".to_string()),
            scope: None,
            top_k: 1,
            as_of: Some(ts(1_700_050_000)),
            include_expired: false,
        })
        .expect("historical retrieval succeeds");

    assert_eq!(
        historical_hits.first().and_then(|hit| hit.value.as_deref()),
        Some("UTC+1")
    );
    assert_eq!(
        historical_hits
            .first()
            .and_then(|hit| hit.metadata.get("stored_at"))
            .map(String::as_str),
        Some(ingested_at.to_rfc3339().as_str())
    );
}

#[test]
fn multiple_access_touches_preserve_historical_visibility() {
    let mut store = InMemoryMemoryStore::default();
    let ingested_at = ts(1_700_000_000);
    let first_access = ts(1_700_050_000);
    let second_access = ts(1_700_100_000);
    let memory = durable(
        "user",
        "timezone",
        "UTC+1",
        "The user usually works in UTC+1",
        MemoryType::Fact,
        SourceType::Chat,
        0.8,
        ingested_at,
    );
    store.put_memory(&memory).expect("memory stored");
    store
        .touch_memory_accesses(&[
            (memory.memory_id.clone(), first_access),
            (memory.memory_id.clone(), second_access),
        ])
        .expect("touches stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "what timezone did the user use earlier".to_string(),
                intent: QueryIntent::HistoricalFact,
                entity: Some("user".to_string()),
                slot: Some("timezone".to_string()),
                scope: None,
                top_k: 1,
                as_of: Some(ts(1_700_010_000)),
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_200_000)),
        )
        .expect("historical retrieval works");

    let hit = hits.first().expect("historical hit exists");
    assert_eq!(hit.value.as_deref(), Some("UTC+1"));
    assert_eq!(
        hit.metadata.get("stored_at").map(String::as_str),
        Some(ingested_at.to_rfc3339().as_str())
    );
}
