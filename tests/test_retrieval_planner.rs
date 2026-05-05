mod common;

use common::{durable, ts};
use memvid_core::agent_memory::adapters::memvid_store::{InMemoryMemoryStore, MemoryStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::{MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::retrieval_candidates::CandidatePool;
use memvid_core::agent_memory::retrieval_planner::RetrievalPlanner;
use memvid_core::agent_memory::schemas::RetrievalQuery;

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

#[test]
fn empty_store_returns_empty_candidates() {
    let mut store = InMemoryMemoryStore::default();
    let clock = FixedClock::new(ts(1_700_000_000));
    let planner = RetrievalPlanner::new();
    let query = base_query("anything");
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    assert!(result.candidates.is_empty());
    assert_eq!(result.pool_stats.lexical_count, 0);
    assert_eq!(result.pool_stats.metadata_count, 0);
    assert_eq!(result.pool_stats.correction_count, 0);
    assert_eq!(result.pool_stats.time_count, 0);
}

#[test]
fn lexical_pool_surfaces_matching_memory() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
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
    let planner = RetrievalPlanner::new();
    let query = base_query("Alice");
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    assert!(
        !result.candidates.is_empty(),
        "lexical pool should surface the memory"
    );
    assert!(result.pool_stats.lexical_count > 0);
    let first = &result.candidates[0];
    assert!(
        first.source_pools.contains(&CandidatePool::Lexical),
        "candidate must be attributed to lexical pool"
    );
}

#[test]
fn metadata_pool_surfaces_entity_slot_match() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
    // MemoryType::Fact → MemoryLayer::Belief, which is in DURABLE_LAYERS
    store
        .put_memory(&durable(
            "user",
            "city",
            "Paris",
            "user lives in Paris",
            MemoryType::Fact,
            SourceType::Chat,
            0.9,
            t,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    // Use a query text that does not match the memory text so only metadata pool fires
    let mut query = base_query("zzz-no-lexical-match-xyz");
    query.entity = Some("user".to_string());
    query.slot = Some("city".to_string());
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    assert!(
        !result.candidates.is_empty(),
        "metadata pool must surface the entity/slot memory"
    );
    assert!(result.pool_stats.metadata_count >= 1);
    let candidate = result
        .candidates
        .iter()
        .find(|c| {
            c.hit.entity.as_deref().map_or(false, |e| e == "user")
                && c.hit.slot.as_deref().map_or(false, |s| s == "city")
        })
        .expect("candidate for user/city must be present");
    assert!(
        candidate.source_pools.contains(&CandidatePool::Metadata),
        "candidate must be attributed to metadata pool"
    );
}

#[test]
fn correction_pool_surfaces_correction_memories() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
    // MemoryType::Correction → MemoryLayer::Correction
    store
        .put_memory(&durable(
            "user",
            "name",
            "Bobby",
            "user name corrected to Bobby",
            MemoryType::Correction,
            SourceType::Chat,
            0.9,
            t,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    let mut query = base_query("zzz-no-lexical-match-xyz");
    query.entity = Some("user".to_string());
    query.slot = Some("name".to_string());
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    assert!(
        !result.candidates.is_empty(),
        "correction pool must surface the correction memory"
    );
    assert_eq!(result.pool_stats.correction_count, 1);
    let candidate = result
        .candidates
        .iter()
        .find(|c| c.source_pools.contains(&CandidatePool::Correction))
        .expect("at least one candidate must be from correction pool");
    assert_eq!(
        candidate.hit.entity.as_deref(),
        Some("user"),
        "correction candidate must have correct entity"
    );
}

#[test]
fn time_pool_surfaces_memories_at_or_before_as_of() {
    let mut store = InMemoryMemoryStore::default();
    let t_early = ts(1_699_000_000);
    let t_now = ts(1_700_000_000);
    let clock = FixedClock::new(t_now);
    // Stored at t_early — so as_of = t_now should include it
    store
        .put_memory(&durable(
            "user",
            "status",
            "online",
            "user was online",
            MemoryType::Trace,
            SourceType::Chat,
            0.9,
            t_early,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    // Use as_of = t_now: memory stored at t_early is included
    let mut query_inclusive = base_query("zzz-no-match");
    query_inclusive.as_of = Some(t_now);
    let result_inclusive = planner.plan(&mut store, &query_inclusive, &clock).unwrap();
    assert!(
        result_inclusive.pool_stats.time_count >= 1,
        "time pool must include memory stored before as_of"
    );
    // Use as_of = t_early - 1 second: memory stored at t_early is excluded
    let t_before_early = ts(t_early.timestamp() - 1);
    let mut query_exclusive = base_query("zzz-no-match");
    query_exclusive.as_of = Some(t_before_early);
    let result_exclusive = planner.plan(&mut store, &query_exclusive, &clock).unwrap();
    assert_eq!(
        result_exclusive.pool_stats.time_count, 0,
        "time pool must exclude memory stored after as_of"
    );
}

#[test]
fn no_entity_slot_skips_metadata_and_correction_pools() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
    store
        .put_memory(&durable(
            "user",
            "name",
            "Alice",
            "user is Alice",
            MemoryType::Fact,
            SourceType::Chat,
            0.9,
            t,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    // No entity, no slot → both metadata and correction pools must be skipped
    let query = base_query("Alice");
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    assert_eq!(
        result.pool_stats.metadata_count, 0,
        "metadata pool must be skipped when no entity/slot on query"
    );
    assert_eq!(
        result.pool_stats.correction_count, 0,
        "correction pool must be skipped when no entity/slot on query"
    );
}

#[test]
fn no_as_of_skips_time_pool() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
    store
        .put_memory(&durable(
            "user",
            "name",
            "Alice",
            "user is Alice",
            MemoryType::Trace,
            SourceType::Chat,
            0.9,
            t,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    // No as_of → time pool must be skipped
    let query = base_query("Alice");
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    assert_eq!(
        result.pool_stats.time_count, 0,
        "time pool must be skipped when query has no as_of"
    );
}

#[test]
fn top_k_truncates_candidates() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
    // Seed 8 memories that all match the lexical query
    for i in 0..8u32 {
        store
            .put_memory(&durable(
                "user",
                &format!("slot{i}"),
                "marvelous",
                &format!("marvelous user slot {i}"),
                MemoryType::Fact,
                SourceType::Chat,
                0.9,
                t,
            ))
            .unwrap();
    }
    let planner = RetrievalPlanner::new();
    let mut query = base_query("marvelous");
    query.top_k = 3;
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    assert!(
        result.candidates.len() <= 3,
        "top_k=3 must limit candidates to at most 3"
    );
}

#[test]
fn planner_does_not_add_memories_to_store() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
    store
        .put_memory(&durable(
            "user",
            "name",
            "Alice",
            "user is Alice",
            MemoryType::Fact,
            SourceType::Chat,
            0.9,
            t,
        ))
        .unwrap();
    let before = store.memories().len();
    let planner = RetrievalPlanner::new();
    let mut query = base_query("Alice");
    query.entity = Some("user".to_string());
    query.slot = Some("name".to_string());
    query.as_of = Some(t);
    planner.plan(&mut store, &query, &clock).unwrap();
    let after = store.memories().len();
    assert_eq!(
        before, after,
        "planner must not insert or remove memories from the store"
    );
}

#[test]
fn multi_pool_candidate_carries_both_source_pools() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
    // "London" appears in the text → lexical pool will surface it.
    // entity="user" slot="city" matches the entity/slot filter → metadata pool fires too.
    store
        .put_memory(&durable(
            "user",
            "city",
            "London",
            "user lives in London",
            MemoryType::Fact, // → MemoryLayer::Belief in DURABLE_LAYERS
            SourceType::Chat,
            0.9,
            t,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    let mut query = base_query("London");
    query.entity = Some("user".to_string());
    query.slot = Some("city".to_string());
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    assert!(!result.candidates.is_empty());
    let candidate = result
        .candidates
        .iter()
        .find(|c| {
            c.hit.entity.as_deref().map_or(false, |e| e == "user")
                && c.hit.slot.as_deref().map_or(false, |s| s == "city")
        })
        .expect("user/city candidate must be present");
    assert!(
        candidate.source_pools.contains(&CandidatePool::Lexical),
        "candidate must carry Lexical pool attribution"
    );
    assert!(
        candidate.source_pools.contains(&CandidatePool::Metadata),
        "candidate must carry Metadata pool attribution"
    );
}
