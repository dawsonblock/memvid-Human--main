mod common;

use memvid_core::agent_memory::adapters::memvid_store::{InMemoryMemoryStore, MemoryStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::consolidation_engine::ConsolidationEngine;
use memvid_core::agent_memory::enums::{MemoryLayer, MemoryType, ProcedureStatus, SourceType};
use memvid_core::agent_memory::episode_store::EpisodeStore;
use memvid_core::agent_memory::memory_retriever::MemoryRetriever;
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::procedure_store::ProcedureStore;
use memvid_core::agent_memory::ranker::Ranker;
use memvid_core::agent_memory::retention::RetentionManager;
use memvid_core::agent_memory::schemas::RetrievalQuery;

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

#[test]
fn task_state_query_downranks_cooling_down_procedures_and_filters_retired_ones() {
    let mut store = InMemoryMemoryStore::default();
    store
        .put_memory(&procedure_memory(
            "procedure-active",
            ProcedureStatus::Active,
            5,
            1,
            ts(1_700_000_000),
        ))
        .expect("active procedure stored");
    store
        .put_memory(&procedure_memory(
            "procedure-cooling",
            ProcedureStatus::CoolingDown,
            2,
            4,
            ts(1_700_000_010),
        ))
        .expect("cooling procedure stored");
    let retired = procedure_memory(
        "procedure-retired",
        ProcedureStatus::Retired,
        1,
        6,
        ts(1_700_000_020),
    );
    let retired_id = retired.memory_id.clone();
    store
        .put_memory(&retired)
        .expect("retired procedure stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "repo_review next steps".to_string(),
                intent: memvid_core::agent_memory::enums::QueryIntent::TaskState,
                entity: None,
                slot: None,
                scope: None,
                top_k: 5,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_060)),
        )
        .expect("retrieval works");

    assert_eq!(
        hits.first().and_then(|hit| hit.memory_id.as_deref()),
        Some("procedure-active")
    );
    assert!(
        hits.iter()
            .all(|hit| hit.memory_id.as_deref() != Some(retired_id.as_str()))
    );
    assert!(
        hits.iter()
            .any(|hit| hit.memory_id.as_deref() == Some("procedure-cooling"))
    );
}

fn procedure_memory(
    memory_id: &str,
    status: ProcedureStatus,
    success_count: u32,
    failure_count: u32,
    stored_at: chrono::DateTime<chrono::Utc>,
) -> memvid_core::agent_memory::schemas::DurableMemory {
    let mut memory = durable(
        "procedure",
        "repo_review",
        "repo_review",
        "Review the repo in a consistent order",
        MemoryType::Trace,
        SourceType::System,
        1.0,
        stored_at,
    );
    memory.memory_id = memory_id.to_string();
    memory.internal_layer = Some(MemoryLayer::Procedure);
    memory
        .metadata
        .insert("procedure_name".to_string(), "repo_review".to_string());
    memory
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    memory
        .metadata
        .insert("context_tags".to_string(), "repo_review,review".to_string());
    memory
        .metadata
        .insert("success_count".to_string(), success_count.to_string());
    memory
        .metadata
        .insert("failure_count".to_string(), failure_count.to_string());
    memory
        .metadata
        .insert("procedure_status".to_string(), status.as_str().to_string());
    memory
}
