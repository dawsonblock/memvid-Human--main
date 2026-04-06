mod common;

use memvid_core::agent_memory::adapters::memvid_store::{InMemoryMemoryStore, MemoryStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::{MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::memory_retriever::MemoryRetriever;
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::ranker::Ranker;
use memvid_core::agent_memory::retention::RetentionManager;
use memvid_core::agent_memory::schemas::RetrievalQuery;

use common::{durable, ts};

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
