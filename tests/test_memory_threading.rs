mod common;

use memvid_core::agent_memory::enums::QueryIntent;
use memvid_core::agent_memory::schemas::RetrievalQuery;

// ────────────────────────────────────────────────────────────────────────────
// Test 1: open_thread returns unique IDs per call.
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn thread_open_returns_unique_ids_per_call() {
    let (mut ctrl, _) = common::controller(common::ts(1_700_000_000));
    let t1 = ctrl.open_thread("user:alice");
    let t2 = ctrl.open_thread("user:alice");
    assert!(!t1.is_empty(), "thread_id must be non-empty");
    assert_ne!(t1, t2, "each open_thread call must produce a unique ID");
}

// ────────────────────────────────────────────────────────────────────────────
// Test 2: attach_to_thread / get_thread_memories round-trip.
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn attach_and_retrieve_thread_memories() {
    let (mut ctrl, _) = common::controller(common::ts(1_700_000_000));
    let tid = ctrl.open_thread("user:bob");

    assert!(
        ctrl.attach_to_thread(&tid, "mem-001"),
        "attach to open thread should succeed"
    );
    assert!(ctrl.attach_to_thread(&tid, "mem-002"));

    let mems = ctrl.get_thread_memories(&tid);
    assert_eq!(mems.len(), 2);
    assert!(mems.contains(&"mem-001"));
    assert!(mems.contains(&"mem-002"));
}

// ────────────────────────────────────────────────────────────────────────────
// Test 3: closing a thread prevents further attachments.
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn closed_thread_rejects_new_attachments() {
    let (mut ctrl, _) = common::controller(common::ts(1_700_000_000));
    let tid = ctrl.open_thread("user:carol");

    assert!(ctrl.attach_to_thread(&tid, "mem-aaa"));
    assert!(ctrl.close_thread(&tid), "first close should return true");
    assert!(
        !ctrl.close_thread(&tid),
        "second close of same thread must return false"
    );

    assert!(
        !ctrl.attach_to_thread(&tid, "mem-bbb"),
        "attach to closed thread must return false"
    );

    let mems = ctrl.get_thread_memories(&tid);
    assert_eq!(mems.len(), 1);
    assert!(mems.contains(&"mem-aaa"));
}

// ────────────────────────────────────────────────────────────────────────────
// Test 4: retrieval filtered by thread_id only returns hits from that thread.
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn retrieval_filters_by_thread_id() {
    use memvid_core::agent_memory::enums::{MemoryType, Scope, SourceType};

    let (mut ctrl, _) = common::controller(common::ts(1_700_000_000));
    let stored_at = common::ts(1_700_000_000);

    let mut mem_alpha = common::durable(
        "user",
        "language",
        "English",
        "user speaks English language",
        MemoryType::Preference,
        SourceType::Chat,
        0.8,
        stored_at,
    );
    mem_alpha.thread_id = Some("thread-alpha".to_string());

    let mut mem_beta = common::durable(
        "user",
        "timezone",
        "UTC",
        "user timezone UTC setting",
        MemoryType::Preference,
        SourceType::Chat,
        0.8,
        stored_at,
    );
    mem_beta.thread_id = Some("thread-beta".to_string());

    common::apply_durable(&mut ctrl, &mem_alpha, None);
    common::apply_durable(&mut ctrl, &mem_beta, None);

    let alpha_hits = ctrl
        .retrieve(RetrievalQuery {
            query_text: "user language English".to_string(),
            intent: QueryIntent::PreferenceLookup,
            entity: Some("user".to_string()),
            slot: None,
            scope: Some(Scope::Private),
            top_k: 10,
            as_of: None,
            include_expired: false,
            namespace_strict: false,
            user_id: None,
            project_id: None,
            task_id: None,
            thread_id: Some("thread-alpha".to_string()),
        })
        .expect("alpha retrieve succeeds");

    assert!(
        alpha_hits
            .iter()
            .all(|h| h.metadata.get("thread_id").map(String::as_str) == Some("thread-alpha")),
        "all alpha hits must belong to thread-alpha; got: {alpha_hits:?}"
    );

    let beta_hits = ctrl
        .retrieve(RetrievalQuery {
            query_text: "user timezone UTC".to_string(),
            intent: QueryIntent::PreferenceLookup,
            entity: Some("user".to_string()),
            slot: None,
            scope: Some(Scope::Private),
            top_k: 10,
            as_of: None,
            include_expired: false,
            namespace_strict: false,
            user_id: None,
            project_id: None,
            task_id: None,
            thread_id: Some("thread-beta".to_string()),
        })
        .expect("beta retrieve succeeds");

    assert!(
        beta_hits
            .iter()
            .all(|h| h.metadata.get("thread_id").map(String::as_str) == Some("thread-beta")),
        "all beta hits must belong to thread-beta; got: {beta_hits:?}"
    );

    let alpha_ids: std::collections::HashSet<_> =
        alpha_hits.iter().filter_map(|h| h.memory_id.as_deref()).collect();
    let beta_ids: std::collections::HashSet<_> =
        beta_hits.iter().filter_map(|h| h.memory_id.as_deref()).collect();
    assert!(
        alpha_ids.is_disjoint(&beta_ids),
        "alpha and beta hit sets must be disjoint"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 5: parent_memory_id is preserved after ingest -> promote.
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn parent_memory_id_propagates_through_pipeline() {
    use memvid_core::agent_memory::enums::{MemoryType, SourceType};

    let (mut ctrl, _) = common::controller(common::ts(1_700_000_000));
    let stored_at = common::ts(1_700_000_000);

    let parent = common::durable(
        "user",
        "name",
        "Alice",
        "user name is Alice",
        MemoryType::Trace,
        SourceType::Chat,
        0.9,
        stored_at,
    );
    let parent_id = common::apply_durable(&mut ctrl, &parent, None);

    let mut child = common::candidate_from_durable(&parent);
    child.candidate_id = format!("child-of-{parent_id}");
    child.raw_text = "user prefers Alice as display name (correction)".to_string();
    child.parent_memory_id = Some(parent_id.clone());

    let child_id = ctrl
        .ingest(child)
        .expect("ingest succeeds")
        .expect("child should be admitted and persisted");

    let child_mem = ctrl
        .store()
        .memories()
        .iter()
        .find(|m| m.memory_id == child_id)
        .expect("child memory must be in the store")
        .clone();

    assert_eq!(
        child_mem.parent_memory_id.as_deref(),
        Some(parent_id.as_str()),
        "parent_memory_id must be preserved after promotion"
    );
}
