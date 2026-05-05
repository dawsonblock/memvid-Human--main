mod common;

use common::{durable, ts};
use memvid_core::agent_memory::adapters::memvid_store::{InMemoryMemoryStore, MemoryStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::{MemoryLayer, MemoryType, QueryIntent, SourceType};
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

/// Memories stored after `as_of` are excluded from the time pool.
#[test]
fn as_of_excludes_memory_stored_after_cutoff() {
    let mut store = InMemoryMemoryStore::default();
    let cutoff = ts(1_700_000_000);
    let after_cutoff = ts(1_700_000_001);
    let clock = FixedClock::new(ts(1_700_001_000));
    // Only memory stored AFTER cutoff — should not appear in time pool
    store
        .put_memory(&durable(
            "user",
            "status",
            "active",
            "user became active",
            MemoryType::Fact, // → MemoryLayer::Belief
            SourceType::System,
            0.9,
            after_cutoff,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    let mut query = base_query("zzz-xno-lexical-match-xzzz");
    query.as_of = Some(cutoff);
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    let time_pool_candidates: Vec<_> = result
        .candidates
        .iter()
        .filter(|c| c.source_pools.contains(&CandidatePool::Time))
        .collect();
    assert!(
        time_pool_candidates.is_empty(),
        "no time-pool candidates expected when all memories stored after as_of"
    );
}

/// Memory stored exactly at `as_of` is included in the time pool (inclusive boundary).
#[test]
fn as_of_includes_memory_stored_at_boundary() {
    let mut store = InMemoryMemoryStore::default();
    let boundary = ts(1_700_000_000);
    let clock = FixedClock::new(ts(1_700_001_000));
    store
        .put_memory(&durable(
            "agent",
            "phase",
            "init",
            "agent entered init phase",
            MemoryType::Episode, // → MemoryLayer::Episode
            SourceType::System,
            0.8,
            boundary, // stored_at == as_of boundary
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    let mut query = base_query("qzz-no-match");
    query.as_of = Some(boundary);
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    let time_candidates: Vec<_> = result
        .candidates
        .iter()
        .filter(|c| c.source_pools.contains(&CandidatePool::Time))
        .collect();
    assert_eq!(
        time_candidates.len(),
        1,
        "memory stored exactly at as_of must be included (inclusive boundary)"
    );
}

/// Correction pool only returns MemoryLayer::Correction memories — not durable layers.
#[test]
fn correction_pool_returns_only_correction_layer_memories() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
    // A regular Fact (→ Belief layer, durable) with matching entity/slot
    store
        .put_memory(&durable(
            "entity1",
            "slot1",
            "original_value",
            "original belief about entity1 slot1",
            MemoryType::Fact,
            SourceType::System,
            0.9,
            t,
        ))
        .unwrap();
    // A Correction (→ Correction layer) with matching entity/slot
    store
        .put_memory(&durable(
            "entity1",
            "slot1",
            "corrected_value",
            "correction: entity1 slot1 is now corrected_value",
            MemoryType::Correction,
            SourceType::Chat,
            0.95,
            t,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    let mut query = base_query("qxx-no-lexical-match");
    query.entity = Some("entity1".to_string());
    query.slot = Some("slot1".to_string());
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    // Every correction-pool candidate must have Correction layer
    for c in result
        .candidates
        .iter()
        .filter(|c| c.source_pools.contains(&CandidatePool::Correction))
    {
        assert_eq!(
            c.layer,
            MemoryLayer::Correction,
            "correction pool must only surface MemoryLayer::Correction memories"
        );
    }
    // Every metadata-pool candidate must NOT have Correction layer
    for c in result
        .candidates
        .iter()
        .filter(|c| c.source_pools.contains(&CandidatePool::Metadata))
    {
        assert_ne!(
            c.layer,
            MemoryLayer::Correction,
            "metadata pool must not surface Correction-layer memories"
        );
    }
}

/// Both the original (durable) memory and the correction are surfaced for the same entity/slot.
#[test]
fn original_and_correction_both_surfaced_for_same_entity_slot() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
    store
        .put_memory(&durable(
            "userX",
            "location",
            "London",
            "userX is in London",
            MemoryType::Fact, // → Belief layer (durable)
            SourceType::Chat,
            0.8,
            t,
        ))
        .unwrap();
    store
        .put_memory(&durable(
            "userX",
            "location",
            "Paris",
            "correction: userX moved to Paris",
            MemoryType::Correction, // → Correction layer
            SourceType::Chat,
            0.95,
            t,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    let mut query = base_query("xxz-no-lexical");
    query.entity = Some("userX".to_string());
    query.slot = Some("location".to_string());
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    let has_metadata = result
        .candidates
        .iter()
        .any(|c| c.source_pools.contains(&CandidatePool::Metadata));
    let has_correction = result
        .candidates
        .iter()
        .any(|c| c.source_pools.contains(&CandidatePool::Correction));
    assert!(
        has_metadata,
        "metadata pool should surface the original Fact memory"
    );
    assert!(
        has_correction,
        "correction pool should surface the Correction memory"
    );
    assert_eq!(
        result.candidates.len(),
        2,
        "exactly 2 memories total: one original + one correction"
    );
}

/// Historical query: early `as_of` returns only the memory stored before that time.
#[test]
fn historical_query_as_of_returns_only_early_memories() {
    let mut store = InMemoryMemoryStore::default();
    let t_early = ts(1_000_000);
    let t_late = ts(2_000_000);
    let cutoff = ts(1_500_000); // between early and late
    let clock = FixedClock::new(ts(3_000_000));
    store
        .put_memory(&durable(
            "proj",
            "version",
            "v1",
            "project version is v1",
            MemoryType::Fact,
            SourceType::System,
            0.9,
            t_early,
        ))
        .unwrap();
    store
        .put_memory(&durable(
            "proj",
            "version",
            "v2",
            "project version is v2",
            MemoryType::Fact,
            SourceType::System,
            0.9,
            t_late,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    let mut query = base_query("xno-lexical-match-yy");
    query.as_of = Some(cutoff);
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    let time_candidates: Vec<_> = result
        .candidates
        .iter()
        .filter(|c| c.source_pools.contains(&CandidatePool::Time))
        .collect();
    assert_eq!(
        time_candidates.len(),
        1,
        "only the early memory (stored before as_of) should appear in the time pool"
    );
    assert!(
        time_candidates[0].hit.text.contains("v1"),
        "the time-pool candidate must be the v1 memory"
    );
}

/// Without `as_of`, the time pool is entirely skipped — no candidates carry Pool::Time.
#[test]
fn no_as_of_means_no_time_pool_candidates() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
    store
        .put_memory(&durable(
            "service",
            "state",
            "running",
            "service is running",
            MemoryType::Fact,
            SourceType::Tool,
            0.9,
            t,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    let query = base_query("xno-match-yy");
    // as_of is None — time pool must not activate
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    for c in &result.candidates {
        assert!(
            !c.source_pools.contains(&CandidatePool::Time),
            "time pool must not activate when as_of is None"
        );
    }
    assert_eq!(
        result.pool_stats.time_count, 0,
        "pool_stats.time_count must be 0"
    );
}

/// Without entity+slot, the correction pool is entirely skipped.
#[test]
fn no_entity_or_slot_means_no_correction_pool_candidates() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
    store
        .put_memory(&durable(
            "task",
            "item",
            "done",
            "task item done correction",
            MemoryType::Correction,
            SourceType::Chat,
            0.9,
            t,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    // entity=None, slot=None → correction pool must not run
    let query = base_query("xno-lexical-match");
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    for c in &result.candidates {
        assert!(
            !c.source_pools.contains(&CandidatePool::Correction),
            "correction pool must not activate without both entity and slot"
        );
    }
    assert_eq!(
        result.pool_stats.correction_count, 0,
        "pool_stats.correction_count must be 0 when entity/slot not provided"
    );
}
