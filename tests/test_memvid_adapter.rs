#![cfg(feature = "lex")]

mod common;

use memvid_core::Memvid;
use memvid_core::agent_memory::adapters::memvid_store::{MemoryStore, MemvidStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::{BeliefStatus, MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::memory_retriever::MemoryRetriever;
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::ranker::Ranker;
use memvid_core::agent_memory::retention::RetentionManager;
use memvid_core::agent_memory::schemas::{BeliefRecord, RetrievalQuery};
use tempfile::tempdir;

use common::{durable, ts};

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
