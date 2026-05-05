mod common;

use common::{durable, ts};
use memvid_core::agent_memory::adapters::memvid_store::{InMemoryMemoryStore, MemoryStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::{MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::memory_retriever::MemoryRetriever;
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::ranker::Ranker;
use memvid_core::agent_memory::retention::RetentionManager;
use memvid_core::agent_memory::retrieval_planner::RetrievalPlanner;
use memvid_core::agent_memory::schemas::RetrievalQuery;

fn retriever() -> MemoryRetriever {
    MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()))
}

fn base_query(text: &str) -> RetrievalQuery {
    RetrievalQuery {
        query_text: text.to_string(),
        intent: QueryIntent::CurrentFact,
        entity: None,
        slot: None,
        scope: None,
        top_k: 10,
        as_of: None,
        include_expired: false,
        namespace_strict: false,
        user_id: None,
        project_id: None,
        task_id: None,
        thread_id: None,
    }
}

/// An empty store should return no hits.
#[test]
fn empty_store_retrieve_returns_empty() {
    let mut store = InMemoryMemoryStore::default();
    let clock = FixedClock::new(ts(1_700_000_000));
    let hits = retriever()
        .retrieve(&mut store, &base_query("anything"), &clock)
        .unwrap();
    assert!(hits.is_empty());
}

/// Retrieve must not write new entries to the store as a side-effect.
#[test]
fn retrieve_does_not_mutate_store() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    store
        .put_memory(&durable(
            "user",
            "name",
            "Alice",
            "The user is Alice",
            MemoryType::Fact,
            SourceType::Chat,
            0.9,
            t,
        ))
        .unwrap();
    store
        .put_memory(&durable(
            "user",
            "role",
            "engineer",
            "user is an engineer",
            MemoryType::Fact,
            SourceType::Chat,
            0.9,
            t,
        ))
        .unwrap();
    let before = store.memories().len();
    let clock = FixedClock::new(ts(1_700_001_000));
    let _ = retriever()
        .retrieve(
            &mut store,
            &RetrievalQuery {
                entity: Some("user".to_string()),
                slot: Some("name".to_string()),
                query_text: "user name".to_string(),
                ..base_query("user name")
            },
            &clock,
        )
        .unwrap();
    assert_eq!(
        store.memories().len(),
        before,
        "retrieve must not write to the store"
    );
}

/// When the planner's intent pool and metadata pool both find the same memory
/// (because the query text overlaps AND entity/slot match), `merge_planner_candidates`
/// should inject the `planner_pools` metadata key into the hit.
#[test]
fn planner_injects_planner_pools_metadata_on_matched_hit() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    store
        .put_memory(&durable(
            "user",
            "editor",
            "neovim",
            "user prefers neovim",
            MemoryType::Preference,
            SourceType::Chat,
            0.9,
            t,
        ))
        .unwrap();
    let clock = FixedClock::new(ts(1_700_001_000));
    let query = RetrievalQuery {
        entity: Some("user".to_string()),
        slot: Some("editor".to_string()),
        query_text: "neovim editor".to_string(),
        ..base_query("neovim editor")
    };
    let hits = retriever().retrieve(&mut store, &query, &clock).unwrap();
    assert!(!hits.is_empty(), "must find the memory");
    let hit = hits
        .iter()
        .find(|h| h.value.as_deref() == Some("neovim"))
        .expect("neovim hit must be present");
    assert!(
        hit.metadata.contains_key("planner_pools"),
        "planner_pools key must be injected by merge_planner_candidates; got metadata: {:?}",
        hit.metadata
    );
}

/// The metadata pool (Pool 3) uses entity/slot filtering — NOT text scoring.
/// A memory whose `raw_text` has zero word-overlap with the query text should
/// still be surfaced when `entity` and `slot` match exactly.
#[test]
fn planner_broadens_results_via_metadata_pool() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    // raw_text is deliberately a nonsense sentinel that cannot overlap with the
    // query tokens below.
    store
        .put_memory(&durable(
            "project",
            "deadline",
            "2025-12-01",
            "xyzzy-planner-broadening-sentinel",
            MemoryType::Fact,
            SourceType::Chat,
            0.85,
            t,
        ))
        .unwrap();
    let clock = FixedClock::new(ts(1_700_001_000));
    let query = RetrievalQuery {
        entity: Some("project".to_string()),
        slot: Some("deadline".to_string()),
        // These tokens have zero overlap with the raw_text above.
        query_text: "qwerty-no-overlap-tokens".to_string(),
        ..base_query("qwerty-no-overlap-tokens")
    };
    let hits = retriever().retrieve(&mut store, &query, &clock).unwrap();
    assert!(
        hits.iter().any(
            |h| h.entity.as_deref() == Some("project") && h.slot.as_deref() == Some("deadline")
        ),
        "metadata pool must surface the memory even when query text has no word-overlap \
         with raw_text; hits returned: {:?}",
        hits
    );
}

/// Historical queries (`as_of.is_some()`) must bypass the planner without
/// panicking or returning an error.
#[test]
fn planner_skipped_for_as_of_historical_queries() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    store
        .put_memory(&durable(
            "user",
            "location",
            "Berlin",
            "user is in Berlin",
            MemoryType::Fact,
            SourceType::Chat,
            0.9,
            t,
        ))
        .unwrap();
    let clock = FixedClock::new(ts(1_700_200_000));
    let query = RetrievalQuery {
        intent: QueryIntent::HistoricalFact,
        entity: Some("user".to_string()),
        slot: Some("location".to_string()),
        query_text: "user location".to_string(),
        as_of: Some(ts(1_700_100_000)),
        top_k: 3,
        ..base_query("user location")
    };
    // Must not panic; the as_of guard silently skips the planner.
    let result = retriever().retrieve(&mut store, &query, &clock);
    assert!(result.is_ok(), "as_of query must not fail: {:?}", result);
}

/// In the default build there is no independent vector index, so
/// `pool_stats.vector_pool_available` must be `false` and a descriptive
/// reason must be provided.
#[test]
fn vector_pool_always_unavailable_in_default_build() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    store
        .put_memory(&durable(
            "agent",
            "model",
            "gpt4",
            "agent uses gpt4",
            MemoryType::Fact,
            SourceType::Chat,
            0.9,
            t,
        ))
        .unwrap();
    let clock = FixedClock::new(ts(1_700_001_000));
    let planner = RetrievalPlanner::new();
    let result = planner
        .plan(&mut store, &base_query("gpt4 model"), &clock)
        .unwrap();
    assert!(
        !result.pool_stats.vector_pool_available,
        "vector pool must always be unavailable in the default build"
    );
    assert!(
        result.pool_stats.vector_pool_skipped_reason.is_some(),
        "a descriptive reason must be provided when vector pool is skipped"
    );
}
