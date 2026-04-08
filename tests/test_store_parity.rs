#![cfg(feature = "lex")]

mod common;

use memvid_core::Memvid;
use memvid_core::agent_memory::adapters::memvid_store::{
    InMemoryMemoryStore, MemoryStore, MemvidStore,
};
use memvid_core::agent_memory::enums::{MemoryType, OutcomeFeedbackKind, QueryIntent, SourceType};
use memvid_core::agent_memory::schemas::{DurableMemory, RetrievalHit, RetrievalQuery};
use tempfile::{TempDir, tempdir};

use common::{durable, ts};

fn parity_stores(name: &str) -> (InMemoryMemoryStore, MemvidStore, TempDir) {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join(format!("{name}.mv2"));
    let memvid = Memvid::create(&path).expect("memvid created");
    (
        InMemoryMemoryStore::default(),
        MemvidStore::new(memvid),
        dir,
    )
}

fn parity_query(as_of: Option<chrono::DateTime<chrono::Utc>>) -> RetrievalQuery {
    RetrievalQuery {
        query_text: "user prefers".to_string(),
        intent: QueryIntent::HistoricalFact,
        entity: Some("user".to_string()),
        slot: Some("favorite_editor".to_string()),
        scope: None,
        top_k: 3,
        as_of,
        include_expired: false,
    }
}

fn assert_effective_memory_match(left: &DurableMemory, right: &DurableMemory) {
    assert_eq!(left.memory_id, right.memory_id);
    assert_eq!(left.stored_at, right.stored_at);
    assert_eq!(left.version_timestamp(), right.version_timestamp());
    assert_eq!(left.value, right.value);
    assert_eq!(left.retrieval_count(), right.retrieval_count());
    assert_eq!(left.last_accessed_at(), right.last_accessed_at());
    assert_eq!(
        left.positive_outcome_count(),
        right.positive_outcome_count()
    );
    assert_eq!(
        left.negative_outcome_count(),
        right.negative_outcome_count()
    );
    assert_eq!(left.last_outcome_at(), right.last_outcome_at());
}

fn assert_hit_match(left: &RetrievalHit, right: &RetrievalHit) {
    assert_eq!(left.memory_id, right.memory_id);
    assert_eq!(left.value, right.value);
    assert_eq!(left.timestamp, right.timestamp);
    assert_eq!(
        left.metadata.get("stored_at"),
        right.metadata.get("stored_at")
    );
    assert_eq!(
        left.metadata.get("retrieval_count"),
        right.metadata.get("retrieval_count")
    );
    assert_eq!(
        left.metadata.get("last_accessed_at"),
        right.metadata.get("last_accessed_at")
    );
}

#[test]
fn stores_match_for_single_access_touch() {
    let (mut in_memory, mut memvid, _dir) = parity_stores("store-parity-single-touch");
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
    in_memory.put_memory(&memory).expect("in-memory stored");
    memvid.put_memory(&memory).expect("memvid stored");

    let accessed_at = ts(1_700_000_100);
    in_memory
        .touch_memory_access(&memory.memory_id, accessed_at)
        .expect("in-memory touch stored");
    memvid
        .touch_memory_access(&memory.memory_id, accessed_at)
        .expect("memvid touch stored");

    let left = in_memory
        .get_memory(&memory.memory_id)
        .expect("in-memory lookup succeeds")
        .expect("in-memory memory exists");
    let right = memvid
        .get_memory(&memory.memory_id)
        .expect("memvid lookup succeeds")
        .expect("memvid memory exists");
    assert_effective_memory_match(&left, &right);
}

#[test]
fn stores_match_for_multiple_access_touches_on_same_memory() {
    let (mut in_memory, mut memvid, _dir) = parity_stores("store-parity-multi-touch");
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
    in_memory.put_memory(&memory).expect("in-memory stored");
    memvid.put_memory(&memory).expect("memvid stored");

    let touches = vec![
        (memory.memory_id.clone(), ts(1_700_000_100)),
        (memory.memory_id.clone(), ts(1_700_000_200)),
    ];
    in_memory
        .touch_memory_accesses(&touches)
        .expect("in-memory touches stored");
    memvid
        .touch_memory_accesses(&touches)
        .expect("memvid touches stored");

    let left = in_memory
        .get_memory(&memory.memory_id)
        .expect("in-memory lookup succeeds")
        .expect("in-memory memory exists");
    let right = memvid
        .get_memory(&memory.memory_id)
        .expect("memvid lookup succeeds")
        .expect("memvid memory exists");
    assert_effective_memory_match(&left, &right);
}

#[test]
fn stores_match_for_touch_then_outcome_feedback() {
    let (mut in_memory, mut memvid, _dir) = parity_stores("store-parity-feedback");
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
    in_memory.put_memory(&memory).expect("in-memory stored");
    memvid.put_memory(&memory).expect("memvid stored");

    let accessed_at = ts(1_700_000_100);
    let feedback_at = ts(1_700_000_200);
    in_memory
        .touch_memory_access(&memory.memory_id, accessed_at)
        .expect("in-memory touch stored");
    memvid
        .touch_memory_access(&memory.memory_id, accessed_at)
        .expect("memvid touch stored");

    let in_memory_updated = in_memory
        .get_memory(&memory.memory_id)
        .expect("in-memory lookup succeeds")
        .expect("in-memory memory exists")
        .with_outcome_feedback(OutcomeFeedbackKind::Positive, feedback_at);
    let memvid_updated = memvid
        .get_memory(&memory.memory_id)
        .expect("memvid lookup succeeds")
        .expect("memvid memory exists")
        .with_outcome_feedback(OutcomeFeedbackKind::Positive, feedback_at);
    in_memory
        .put_memory(&in_memory_updated)
        .expect("in-memory feedback stored");
    memvid
        .put_memory(&memvid_updated)
        .expect("memvid feedback stored");

    let left = in_memory
        .get_memory(&memory.memory_id)
        .expect("in-memory lookup succeeds")
        .expect("in-memory memory exists");
    let right = memvid
        .get_memory(&memory.memory_id)
        .expect("memvid lookup succeeds")
        .expect("memvid memory exists");
    assert_effective_memory_match(&left, &right);
}

#[test]
fn stores_match_for_historical_as_of_queries_after_access_touches() {
    let (mut in_memory, mut memvid, _dir) = parity_stores("store-parity-historical");
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
    in_memory.put_memory(&memory).expect("in-memory stored");
    memvid.put_memory(&memory).expect("memvid stored");

    in_memory
        .touch_memory_accesses(&[(memory.memory_id.clone(), ts(1_700_000_100))])
        .expect("first in-memory touch stored");
    in_memory
        .touch_memory_accesses(&[(memory.memory_id.clone(), ts(1_700_000_200))])
        .expect("second in-memory touch stored");
    memvid
        .touch_memory_accesses(&[(memory.memory_id.clone(), ts(1_700_000_100))])
        .expect("first memvid touch stored");
    memvid
        .touch_memory_accesses(&[(memory.memory_id.clone(), ts(1_700_000_200))])
        .expect("second memvid touch stored");

    let left = in_memory.search(&parity_query(Some(ts(1_700_000_050))));
    let right = memvid.search(&parity_query(Some(ts(1_700_000_050))));
    let left_hit = left
        .expect("in-memory search succeeds")
        .into_iter()
        .find(|hit| hit.memory_id.as_deref() == Some(memory.memory_id.as_str()))
        .expect("in-memory historical hit exists");
    let right_hit = right
        .expect("memvid search succeeds")
        .into_iter()
        .find(|hit| hit.memory_id.as_deref() == Some(memory.memory_id.as_str()))
        .expect("memvid historical hit exists");
    assert_hit_match(&left_hit, &right_hit);
}
