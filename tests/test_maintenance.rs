#![cfg(feature = "lex")]

mod common;

use memvid_core::agent_memory::adapters::memvid_store::MemoryStore;
use memvid_core::agent_memory::enums::{MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::memory_compactor::MemoryCompactor;
use memvid_core::agent_memory::schemas::RetrievalQuery;

use common::{apply_durable, controller, durable, ts};

#[test]
fn maintenance_reports_current_memories_expires_due_entries_and_audits_activity() {
    let now = ts(1_700_000_200);
    let (mut controller, sink) = controller(now);

    let active = durable(
        "user",
        "favorite_editor",
        "vim",
        "The user prefers vim for editing",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    let mut expired = durable(
        "task",
        "status",
        "stale",
        "This stale task-state record should expire",
        MemoryType::Episode,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    expired.ttl = Some(60);

    let active_id = apply_durable(&mut controller, &active, None);
    let expired_id = apply_durable(&mut controller, &expired, None);
    controller
        .store_mut()
        .touch_memory_access(&active_id, ts(1_700_000_150))
        .expect("touch stored");

    let before = controller
        .store_mut()
        .get_memory(&active_id)
        .expect("lookup succeeds")
        .expect("active memory exists");
    assert_eq!(before.retrieval_count(), 1);
    assert_eq!(before.last_accessed_at(), Some(ts(1_700_000_150)));

    let report = controller.run_maintenance().expect("maintenance succeeds");

    assert_eq!(report.durable_memories.len(), 2);
    assert!(
        report
            .durable_memories
            .iter()
            .any(|memory| memory.memory_id == active_id)
    );
    assert!(
        report
            .durable_memories
            .iter()
            .any(|memory| memory.memory_id == expired_id)
    );
    assert_eq!(report.expired_ids, vec![expired_id.clone()]);
    assert!(!report.compaction_supported);
    assert_eq!(report.compactor_status, "unsupported");
    assert_eq!(
        report.compactor_reason,
        MemoryCompactor.unsupported_reason()
    );

    let after = controller
        .store_mut()
        .get_memory(&active_id)
        .expect("lookup succeeds")
        .expect("active memory exists");
    assert_eq!(after.retrieval_count(), 1);
    assert_eq!(after.last_accessed_at(), Some(ts(1_700_000_150)));

    let visible_hits = controller
        .store_mut()
        .search(&RetrievalQuery {
            query_text: "stale task-state".to_string(),
            intent: QueryIntent::EpisodicRecall,
            entity: None,
            slot: None,
            scope: None,
            top_k: 5,
            as_of: None,
            include_expired: false,
        })
        .expect("search succeeds");
    assert!(
        visible_hits
            .iter()
            .all(|hit| hit.memory_id.as_deref() != Some(expired_id.as_str()))
    );

    let maintenance_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "maintenance")
        .expect("maintenance audit event present");
    assert_eq!(
        maintenance_event
            .details
            .get("durable_memory_count")
            .map(String::as_str),
        Some("2")
    );
    assert_eq!(
        maintenance_event
            .details
            .get("expired_ids")
            .map(String::as_str),
        Some(expired_id.as_str())
    );
    assert_eq!(
        maintenance_event
            .details
            .get("compactor_status")
            .map(String::as_str),
        Some("unsupported")
    );
    assert_eq!(
        maintenance_event
            .details
            .get("compactor_reason")
            .map(String::as_str),
        Some(MemoryCompactor.unsupported_reason())
    );
}
