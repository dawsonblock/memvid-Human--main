#![cfg(feature = "lex")]

mod common;

use memvid_core::agent_memory::enums::{MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::schemas::RetrievalQuery;

use common::{apply_durable, controller_with_policy, durable, ts};

/// Even when touch persistence is fully opted in (policy + store both consent), a query
/// that carries `as_of: Some(...)` must never write touch metadata.
///
/// Historical reads are always read-only: the `as_of` guard in `touch_retrieved_memories`
/// must fire before any store mutation occurs.
#[test]
fn historical_retrieval_does_not_write_touch_metadata_even_when_fully_opted_in() {
    let stored_at = ts(1_700_000_000);
    // as_of sits between stored_at and now so the memory is in the historical window.
    let as_of_time = ts(1_700_000_050);
    let now = ts(1_700_000_100);

    // Fully opted-in: the policy would normally allow touch writes.
    let (mut controller, sink) = controller_with_policy(
        now,
        PolicySet::default().with_persist_retrieval_touches(true),
    );

    let memory = durable(
        "user",
        "favorite_shell",
        "zsh",
        "The user prefers zsh as their shell",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        stored_at,
    );
    let memory_id = apply_durable(&mut controller, &memory, None);

    controller
        .retrieve(RetrievalQuery {
            query_text: "what shell does the user prefer".to_string(),
            intent: QueryIntent::PreferenceLookup,
            entity: Some("user".to_string()),
            slot: None,
            scope: None,
            top_k: 1,
            as_of: Some(as_of_time), // historical query
            include_expired: false,
            namespace_strict: false,
            user_id: None,
            project_id: None,
            task_id: None,
            thread_id: None,
        })
        .expect("historical retrieval succeeds");

    // No touch must have been applied despite the policy opting in.
    let latest = controller
        .get_memory_by_id(&memory_id)
        .expect("lookup succeeds")
        .expect("memory present");

    assert_eq!(
        latest.version_timestamp(),
        stored_at,
        "version_timestamp must not change on a historical (as_of) read"
    );
    assert!(
        latest.metadata.get("retrieval_count").is_none(),
        "retrieval_count must not be written on a historical read"
    );
    assert!(
        latest.metadata.get("last_accessed_at").is_none(),
        "last_accessed_at must not be written on a historical read"
    );

    // Audit must reflect that no touches were issued.
    let retrieval_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "retrieval")
        .expect("retrieval audit event present");
    assert!(
        !retrieval_event.details.contains_key("touched_memory_ids"),
        "touched_memory_ids must be absent on a historical (as_of) retrieval"
    );
    assert!(
        !retrieval_event.details.contains_key("touched_memories"),
        "touched_memories must be absent on a historical (as_of) retrieval"
    );
}
