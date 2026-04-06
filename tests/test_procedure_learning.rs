mod common;

use memvid_core::agent_memory::adapters::memvid_store::InMemoryMemoryStore;
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::consolidation_engine::ConsolidationEngine;
use memvid_core::agent_memory::enums::{MemoryLayer, MemoryType, ProcedureStatus, SourceType};
use memvid_core::agent_memory::episode_store::EpisodeStore;
use memvid_core::agent_memory::procedure_store::ProcedureStore;

use common::{durable, ts};

#[test]
fn repeated_successful_workflows_promote_into_procedure_memory() {
    let mut store = InMemoryMemoryStore::default();
    let mut first = durable(
        "project",
        "event",
        "repo_review",
        "Completed the repo review workflow successfully",
        MemoryType::Episode,
        SourceType::Tool,
        0.8,
        ts(1_700_000_000),
    );
    first
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    first
        .metadata
        .insert("outcome".to_string(), "success".to_string());
    first.metadata.insert(
        "procedure_description".to_string(),
        "Start with blockers, then validate runtime surface, then check tests.".to_string(),
    );

    let mut second = durable(
        "project",
        "event",
        "repo_review",
        "Completed the repo review workflow successfully again",
        MemoryType::Episode,
        SourceType::Tool,
        0.8,
        ts(1_700_000_060),
    );
    second
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    second
        .metadata
        .insert("outcome".to_string(), "success".to_string());
    second.metadata.insert(
        "procedure_description".to_string(),
        "Start with blockers, then validate runtime surface, then check tests.".to_string(),
    );

    {
        let mut episode_store = EpisodeStore::new(&mut store);
        episode_store
            .save_memory(&first)
            .expect("first episode stored");
    }
    let first_outcomes = ConsolidationEngine
        .consolidate(
            &mut store,
            Some(&first),
            None,
            &FixedClock::new(ts(1_700_000_001)),
        )
        .expect("first consolidation succeeds");
    assert!(first_outcomes.is_empty());

    {
        let mut episode_store = EpisodeStore::new(&mut store);
        episode_store
            .save_memory(&second)
            .expect("second episode stored");
    }
    let second_outcomes = ConsolidationEngine
        .consolidate(
            &mut store,
            Some(&second),
            None,
            &FixedClock::new(ts(1_700_000_061)),
        )
        .expect("second consolidation succeeds");

    let learned_procedure = {
        let mut procedure_store = ProcedureStore::new(&mut store);
        procedure_store
            .get_by_workflow_key("repo_review")
            .expect("procedure lookup succeeds")
            .expect("procedure exists")
    };

    assert_eq!(second_outcomes.len(), 1);
    assert_eq!(
        second_outcomes[0].record.target_layer,
        MemoryLayer::Procedure
    );
    assert_eq!(learned_procedure.status, ProcedureStatus::Active);
    assert_eq!(learned_procedure.success_count, 2);
}
