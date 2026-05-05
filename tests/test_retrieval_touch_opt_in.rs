#![cfg(feature = "lex")]

mod common;

use memvid_core::agent_memory::enums::{MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::schemas::RetrievalQuery;

use common::{apply_durable, controller, controller_with_policy, durable, ts};

/// Without opting in, the default policy leaves memory unmodified after retrieval.
#[test]
fn retrieval_without_opt_in_leaves_memory_version_timestamp_unchanged() {
    let stored_at = ts(1_700_000_000);
    let now = ts(1_700_000_100);
    let (mut controller, _sink) = controller(now);

    let memory = durable(
        "user",
        "theme",
        "dark",
        "The user prefers dark mode",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        stored_at,
    );
    let memory_id = apply_durable(&mut controller, &memory, None);

    controller
        .retrieve(RetrievalQuery {
            query_text: "user theme preference".to_string(),
            intent: QueryIntent::PreferenceLookup,
            entity: Some("user".to_string()),
            slot: None,
            scope: None,
            top_k: 1,
            as_of: None,
            include_expired: false,
            namespace_strict: false,
            user_id: None,
            project_id: None,
            task_id: None,
            thread_id: None,
        })
        .expect("retrieval succeeds");

    let latest = controller
        .get_memory_by_id(&memory_id)
        .expect("lookup succeeds")
        .expect("memory present");
    assert_eq!(latest.version_timestamp(), stored_at);
    assert!(latest.metadata.get("retrieval_count").is_none());
    assert!(latest.metadata.get("last_accessed_at").is_none());
}

/// When the caller explicitly opts in via `with_persist_retrieval_touches(true)`, a retrieval
/// must write touch metadata and update version_timestamp to the retrieval time.
#[test]
fn retrieval_with_opt_in_writes_touch_metadata_and_updates_version_timestamp() {
    let stored_at = ts(1_700_000_000);
    let now = ts(1_700_000_100);
    let (mut controller, sink) = controller_with_policy(
        now,
        PolicySet::default().with_persist_retrieval_touches(true),
    );

    let memory = durable(
        "user",
        "theme",
        "dark",
        "The user prefers dark mode",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        stored_at,
    );
    let memory_id = apply_durable(&mut controller, &memory, None);

    controller
        .retrieve(RetrievalQuery {
            query_text: "user theme preference".to_string(),
            intent: QueryIntent::PreferenceLookup,
            entity: Some("user".to_string()),
            slot: None,
            scope: None,
            top_k: 1,
            as_of: None,
            include_expired: false,
            namespace_strict: false,
            user_id: None,
            project_id: None,
            task_id: None,
            thread_id: None,
        })
        .expect("retrieval succeeds");

    let latest = controller
        .get_memory_by_id(&memory_id)
        .expect("lookup succeeds")
        .expect("memory present");
    assert_eq!(
        latest.version_timestamp(),
        now,
        "touch must update version_timestamp to retrieval time"
    );
    assert_eq!(
        latest.metadata.get("retrieval_count").map(String::as_str),
        Some("1"),
        "retrieval_count must be 1 after one retrieval with touch opted in"
    );
    assert_eq!(
        latest.metadata.get("last_accessed_at").map(String::as_str),
        Some(now.to_rfc3339().as_str()),
    );

    let retrieval_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "retrieval")
        .expect("retrieval audit event present");
    assert_eq!(
        retrieval_event
            .details
            .get("touch_persistence")
            .map(String::as_str),
        Some("enabled")
    );
    assert_eq!(
        retrieval_event
            .details
            .get("touched_memories")
            .map(String::as_str),
        Some("1")
    );
}
