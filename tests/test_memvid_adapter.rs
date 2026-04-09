#![cfg(feature = "lex")]

mod common;

use memvid_core::Memvid;
use memvid_core::agent_memory::adapters::memvid_store::{MemoryStore, MemvidStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::OutcomeFeedbackKind;
use memvid_core::agent_memory::enums::{BeliefStatus, MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::memory_retriever::MemoryRetriever;
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::ranker::Ranker;
use memvid_core::agent_memory::retention::RetentionManager;
use memvid_core::agent_memory::schemas::{BeliefRecord, RetrievalQuery};
use tempfile::tempdir;

use common::{durable, ts};

const ACCESS_ENTITY: &str = "__agent_memory_access__";

fn access_touch_count(path: &std::path::Path, memory_id: &str) -> usize {
    let memvid = Memvid::open(path).expect("memvid reopened");
    memvid.memories().get_cards(ACCESS_ENTITY, memory_id).len()
}

#[test]
fn memvid_adapter_maps_governed_memory_to_real_memvid_interfaces() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("agent-memory.mv2");
    let memvid = Memvid::create(&path).expect("memvid created");
    let mut store = MemvidStore::new(memvid);

    let memory = durable(
        "user",
        "location",
        "Berlin",
        "The user lives in Berlin",
        memvid_core::agent_memory::enums::MemoryType::Fact,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    store.put_memory(&memory).expect("memory stored");
    store
        .update_belief(&BeliefRecord {
            belief_id: "belief-1".to_string(),
            entity: "user".to_string(),
            slot: "location".to_string(),
            current_value: "Berlin".to_string(),
            status: BeliefStatus::Active,
            confidence: 0.9,
            valid_from: ts(1_700_000_000),
            valid_to: None,
            last_reviewed_at: ts(1_700_000_010),
            supporting_memory_ids: vec![memory.memory_id.clone()],
            opposing_memory_ids: Vec::new(),
            contradictions_observed: 0,
            last_contradiction_at: None,
            time_to_last_resolution_seconds: None,
            positive_outcome_count: 0,
            negative_outcome_count: 0,
            last_outcome_at: None,
            source_weights: std::collections::BTreeMap::from([(SourceType::Chat, 0.75)]),
        })
        .expect("belief stored");

    let belief = store
        .get_active_belief("user", "location")
        .expect("belief lookup works")
        .expect("active belief exists");
    let histories = store
        .list_memories_for_belief("user", "location")
        .expect("history lookup works");
    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "what is the user's current location".to_string(),
                intent: QueryIntent::CurrentFact,
                entity: Some("user".to_string()),
                slot: Some("location".to_string()),
                scope: None,
                top_k: 3,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_020)),
        )
        .expect("retrieval works");

    assert_eq!(belief.current_value, "Berlin");
    assert_eq!(histories.len(), 1);
    assert!(!hits.is_empty());
}

#[test]
fn memvid_adapter_search_hits_surface_internal_memory_layer() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("agent-memory-layers.mv2");
    let memvid = Memvid::create(&path).expect("memvid created");
    let mut store = MemvidStore::new(memvid);

    let memory = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    store.put_memory(&memory).expect("memory stored");

    let hits = store
        .search(&RetrievalQuery {
            query_text: "concise responses".to_string(),
            intent: QueryIntent::PreferenceLookup,
            entity: Some("user".to_string()),
            slot: None,
            scope: None,
            top_k: 5,
            as_of: None,
            include_expired: false,
        })
        .expect("search works");

    let hit = hits
        .into_iter()
        .find(|hit| hit.memory_id.as_deref() == Some(memory.memory_id.as_str()))
        .expect("stored memory hit present");

    assert_eq!(
        hit.metadata.get("memory_layer").map(String::as_str),
        Some("self_model")
    );
}

#[test]
fn memvid_adapter_keeps_ingest_time_stable_across_access_and_feedback_updates() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("agent-memory-updates.mv2");
    let memvid = Memvid::create(&path).expect("memvid created");
    let mut store = MemvidStore::new(memvid);

    let stored_at = ts(1_700_000_000);
    let accessed_at = ts(1_700_000_100);
    let feedback_at = ts(1_700_000_200);
    let memory = durable(
        "user",
        "location",
        "Berlin",
        "The user lives in Berlin",
        MemoryType::Fact,
        SourceType::Chat,
        0.75,
        stored_at,
    );
    store.put_memory(&memory).expect("memory stored");

    store
        .touch_memory_access(&memory.memory_id, accessed_at)
        .expect("touch stored");

    let touched = store
        .get_memory(&memory.memory_id)
        .expect("lookup succeeds")
        .expect("memory exists");
    assert_eq!(touched.stored_at, stored_at);
    assert_eq!(touched.version_timestamp(), accessed_at);
    assert_eq!(touched.retrieval_count(), 1);
    assert_eq!(
        store
            .list_memories_by_layer(touched.memory_layer())
            .expect("list succeeds")
            .len(),
        1
    );

    let updated = touched.with_outcome_feedback(OutcomeFeedbackKind::Positive, feedback_at);
    store.put_memory(&updated).expect("feedback version stored");

    let latest = store
        .get_memory(&memory.memory_id)
        .expect("lookup succeeds")
        .expect("memory exists");
    assert_eq!(latest.stored_at, stored_at);
    assert_eq!(latest.version_timestamp(), feedback_at);
    assert_eq!(latest.positive_outcome_count(), 1);

    let historical_hits = store
        .search(&RetrievalQuery {
            query_text: "Berlin".to_string(),
            intent: QueryIntent::CurrentFact,
            entity: Some("user".to_string()),
            slot: Some("location".to_string()),
            scope: None,
            top_k: 3,
            as_of: Some(ts(1_700_000_150)),
            include_expired: false,
        })
        .expect("search works");
    let historical = historical_hits
        .into_iter()
        .find(|hit| hit.memory_id.as_deref() == Some(memory.memory_id.as_str()))
        .expect("historical hit exists");
    assert_eq!(
        historical.metadata.get("stored_at").map(String::as_str),
        Some(stored_at.to_rfc3339().as_str())
    );
}

#[test]
fn memvid_adapter_batch_touch_path_updates_effective_access_metadata() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("agent-memory-batch-touch.mv2");
    let memvid = Memvid::create(&path).expect("memvid created");
    let mut store = MemvidStore::new(memvid);

    let first = durable(
        "user",
        "favorite_editor",
        "vim",
        "The user prefers vim for editing",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    let second = durable(
        "user",
        "favorite_shell",
        "fish",
        "The user prefers fish for shell work",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_010),
    );
    store.put_memory(&first).expect("first memory stored");
    store.put_memory(&second).expect("second memory stored");

    let first_touch = ts(1_700_000_100);
    let second_touch = ts(1_700_000_110);
    let first_touch_again = ts(1_700_000_120);
    store
        .touch_memory_accesses(&[
            (first.memory_id.clone(), first_touch),
            (second.memory_id.clone(), second_touch),
            (first.memory_id.clone(), first_touch_again),
        ])
        .expect("batch touch stored");

    let first_latest = store
        .get_memory(&first.memory_id)
        .expect("first lookup succeeds")
        .expect("first memory exists");
    let second_latest = store
        .get_memory(&second.memory_id)
        .expect("second lookup succeeds")
        .expect("second memory exists");

    assert_eq!(first_latest.retrieval_count(), 2);
    assert_eq!(first_latest.last_accessed_at(), Some(first_touch_again));
    assert_eq!(second_latest.retrieval_count(), 1);
    assert_eq!(second_latest.last_accessed_at(), Some(second_touch));

    let hits = store
        .search(&RetrievalQuery {
            query_text: "user prefers".to_string(),
            intent: QueryIntent::PreferenceLookup,
            entity: Some("user".to_string()),
            slot: None,
            scope: None,
            top_k: 5,
            as_of: None,
            include_expired: false,
        })
        .expect("search works");
    let first_hit = hits
        .iter()
        .find(|hit| hit.memory_id.as_deref() == Some(first.memory_id.as_str()))
        .expect("first hit present");
    let second_hit = hits
        .iter()
        .find(|hit| hit.memory_id.as_deref() == Some(second.memory_id.as_str()))
        .expect("second hit present");

    assert_eq!(
        first_hit
            .metadata
            .get("retrieval_count")
            .map(String::as_str),
        Some("2")
    );
    assert_eq!(
        second_hit
            .metadata
            .get("retrieval_count")
            .map(String::as_str),
        Some("1")
    );
}

#[test]
fn memvid_adapter_can_disable_durable_touch_persistence() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("agent-memory-disabled-touch.mv2");
    let memvid = Memvid::create(&path).expect("memvid created");
    let mut store = MemvidStore::with_access_touch_persistence(memvid, false);

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
    store.put_memory(&memory).expect("memory stored");

    store
        .touch_memory_accesses(&[(memory.memory_id.clone(), ts(1_700_000_100))])
        .expect("touch call succeeds");

    let latest = store
        .get_memory(&memory.memory_id)
        .expect("lookup succeeds")
        .expect("memory exists");
    assert_eq!(latest.retrieval_count(), 0);
    assert_eq!(latest.last_accessed_at(), None);

    drop(store);
    assert_eq!(access_touch_count(&path, &memory.memory_id), 0);
}

#[test]
fn memvid_adapter_persists_durable_touch_records_when_enabled() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("agent-memory-enabled-touch.mv2");
    let memvid = Memvid::create(&path).expect("memvid created");
    let mut store = MemvidStore::new(memvid);

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
    store.put_memory(&memory).expect("memory stored");

    store
        .touch_memory_access(&memory.memory_id, ts(1_700_000_100))
        .expect("first touch stored");
    store
        .touch_memory_access(&memory.memory_id, ts(1_700_000_120))
        .expect("second touch stored");

    let latest = store
        .get_memory(&memory.memory_id)
        .expect("lookup succeeds")
        .expect("memory exists");
    assert_eq!(latest.retrieval_count(), 2);
    assert_eq!(latest.last_accessed_at(), Some(ts(1_700_000_120)));

    drop(store);
    assert_eq!(access_touch_count(&path, &memory.memory_id), 2);
}

#[test]
fn memvid_adapter_cache_tracks_touch_updates_without_stale_reads() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("agent-memory-cache.mv2");
    let memvid = Memvid::create(&path).expect("memvid created");
    let mut store = MemvidStore::new(memvid);

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
    store.put_memory(&memory).expect("memory stored");

    let initial = store
        .get_memory(&memory.memory_id)
        .expect("initial lookup succeeds")
        .expect("memory exists");
    assert_eq!(initial.retrieval_count(), 0);
    assert_eq!(initial.last_accessed_at(), None);

    store
        .touch_memory_accesses(&[(memory.memory_id.clone(), ts(1_700_000_100))])
        .expect("touch stored");
    let first_read = store
        .get_memory(&memory.memory_id)
        .expect("first read succeeds")
        .expect("memory exists");
    assert_eq!(first_read.retrieval_count(), 1);
    assert_eq!(first_read.last_accessed_at(), Some(ts(1_700_000_100)));

    store
        .touch_memory_accesses(&[(memory.memory_id.clone(), ts(1_700_000_120))])
        .expect("second touch stored");
    let second_read = store
        .get_memory(&memory.memory_id)
        .expect("second read succeeds")
        .expect("memory exists");
    let repeated_read = store
        .get_memory(&memory.memory_id)
        .expect("repeated read succeeds")
        .expect("memory exists");

    assert_eq!(second_read.retrieval_count(), 2);
    assert_eq!(second_read.last_accessed_at(), Some(ts(1_700_000_120)));
    assert_eq!(repeated_read.retrieval_count(), 2);
    assert_eq!(repeated_read.last_accessed_at(), Some(ts(1_700_000_120)));
}

#[test]
fn memvid_adapter_preserves_feedback_then_touch_order_in_effective_metadata() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("agent-memory-feedback-touch-order.mv2");
    let memvid = Memvid::create(&path).expect("memvid created");
    let mut store = MemvidStore::new(memvid);

    let stored_at = ts(1_700_000_000);
    let feedback_at = ts(1_700_000_100);
    let touched_at = ts(1_700_000_120);
    let memory = durable(
        "user",
        "favorite_editor",
        "vim",
        "The user prefers vim for editing",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        stored_at,
    );
    store.put_memory(&memory).expect("memory stored");

    let _ = store
        .get_memory(&memory.memory_id)
        .expect("initial lookup succeeds");

    let updated = memory
        .clone()
        .with_outcome_feedback(OutcomeFeedbackKind::Positive, feedback_at);
    store.put_memory(&updated).expect("feedback version stored");
    store
        .touch_memory_accesses(&[(memory.memory_id.clone(), touched_at)])
        .expect("touch stored");

    let latest = store
        .get_memory(&memory.memory_id)
        .expect("lookup succeeds")
        .expect("memory exists");
    let repeated = store
        .get_memory(&memory.memory_id)
        .expect("repeated lookup succeeds")
        .expect("memory exists");

    assert_eq!(latest.positive_outcome_count(), 1);
    assert_eq!(latest.retrieval_count(), 1);
    assert_eq!(latest.last_accessed_at(), Some(touched_at));
    assert_eq!(latest.version_timestamp(), touched_at);
    assert_eq!(repeated.positive_outcome_count(), 1);
    assert_eq!(repeated.retrieval_count(), 1);
    assert_eq!(repeated.last_accessed_at(), Some(touched_at));
    assert_eq!(repeated.version_timestamp(), touched_at);
}
