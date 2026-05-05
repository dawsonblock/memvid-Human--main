#![cfg(feature = "lex")]

mod common;

use memvid_core::agent_memory::enums::{MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::schemas::RetrievalQuery;

use common::{apply_durable, controller, durable, ts};

/// With the default `PolicySet` (persist_retrieval_touches: false), a retrieval must not
/// write any touch metadata to the memory store. The memory's version_timestamp must remain
/// equal to stored_at and no retrieval_count / last_accessed_at metadata keys must appear.
#[test]
fn retrieval_does_not_write_touch_metadata_with_default_policy() {
    let stored_at = ts(1_700_000_000);
    let now = ts(1_700_000_100);
    let (mut controller, sink) = controller(now);

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

    let memory_id = apply_durable(&mut controller, &memory, None);
    assert_eq!(controller.store().memories().len(), 1);

    controller
        .retrieve(RetrievalQuery {
            query_text: "what editor does the user prefer".to_string(),
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

    // version_timestamp must be unchanged — no write side-effect.
    let latest = controller
        .get_memory_by_id(&memory_id)
        .expect("lookup succeeds")
        .expect("memory present");

    assert_eq!(
        latest.version_timestamp(),
        stored_at,
        "version_timestamp must not change when touch persistence is disabled"
    );
    assert!(
        latest.metadata.get("retrieval_count").is_none(),
        "retrieval_count must be absent when touch persistence is disabled"
    );
    assert!(
        latest.metadata.get("last_accessed_at").is_none(),
        "last_accessed_at must be absent when touch persistence is disabled"
    );

    // Audit must record touch_persistence == "disabled" with no touched_memory_ids.
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
        Some("disabled")
    );
    assert!(
        !retrieval_event.details.contains_key("touched_memories"),
        "touched_memories key must be absent when touch is disabled"
    );
    assert!(
        !retrieval_event.details.contains_key("touched_memory_ids"),
        "touched_memory_ids key must be absent when touch is disabled"
    );
}
