mod common;

use memvid_core::agent_memory::adapters::memvid_store::{InMemoryMemoryStore, MemoryStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::consolidation_engine::ConsolidationEngine;
use memvid_core::agent_memory::enums::{
    MemoryLayer, MemoryType, ProcedureStatus, Scope, SourceType,
};
use memvid_core::agent_memory::episode_store::EpisodeStore;
use memvid_core::agent_memory::memory_retriever::MemoryRetriever;
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::procedure_store::ProcedureStore;
use memvid_core::agent_memory::ranker::Ranker;
use memvid_core::agent_memory::retention::RetentionManager;
use memvid_core::agent_memory::schemas::{CandidateMemory, Provenance, RetrievalQuery};

use common::{candidate, controller, durable, ts};

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
    let first_outcomes = ConsolidationEngine::default()
        .consolidate(
            &mut store,
            Some(&first),
            None,
            &FixedClock::new(ts(1_700_000_001)),
        )
        .expect("first consolidation succeeds");
    assert!(first_outcomes.is_empty());
    {
        let mut procedure_store = ProcedureStore::new(&mut store);
        assert!(
            procedure_store
                .get_by_workflow_key("repo_review")
                .expect("procedure lookup succeeds")
                .is_none()
        );
    }

    {
        let mut episode_store = EpisodeStore::new(&mut store);
        episode_store
            .save_memory(&second)
            .expect("second episode stored");
    }
    let second_outcomes = ConsolidationEngine::default()
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

    let rerun_outcomes = ConsolidationEngine::default()
        .consolidate(
            &mut store,
            Some(&second),
            None,
            &FixedClock::new(ts(1_700_000_062)),
        )
        .expect("rerun consolidation succeeds");
    assert!(rerun_outcomes.is_empty());
    let rerun_procedure = {
        let mut procedure_store = ProcedureStore::new(&mut store);
        procedure_store
            .get_by_workflow_key("repo_review")
            .expect("procedure lookup succeeds")
            .expect("procedure still exists")
    };
    assert_eq!(rerun_procedure.success_count, 2);
}

#[test]
fn task_state_query_downranks_cooling_down_procedures_and_filters_retired_ones() {
    let mut store = InMemoryMemoryStore::default();
    let active = procedure_memory(
        "procedure-active",
        ProcedureStatus::Active,
        5,
        1,
        ts(1_700_000_000),
    );
    store.put_memory(&active).expect("active procedure stored");
    let mut cooling = procedure_memory(
        "procedure-cooling",
        ProcedureStatus::CoolingDown,
        2,
        4,
        ts(1_700_000_010),
    );
    cooling.slot = "review_backup".to_string();
    cooling.value = "review_backup".to_string();
    cooling
        .metadata
        .insert("workflow_key".to_string(), "review_backup".to_string());
    cooling
        .metadata
        .insert("procedure_name".to_string(), "review_backup".to_string());
    cooling
        .metadata
        .insert("context_tags".to_string(), "review,backup".to_string());
    store
        .put_memory(&cooling)
        .expect("cooling procedure stored");
    let mut retired = procedure_memory(
        "procedure-retired",
        ProcedureStatus::Retired,
        1,
        6,
        ts(1_700_000_020),
    );
    retired.slot = "retired_review".to_string();
    retired.value = "retired_review".to_string();
    retired
        .metadata
        .insert("workflow_key".to_string(), "retired_review".to_string());
    retired
        .metadata
        .insert("procedure_name".to_string(), "retired_review".to_string());
    retired
        .metadata
        .insert("context_tags".to_string(), "review,retired".to_string());
    let retired_id = retired.memory_id.clone();
    store
        .put_memory(&retired)
        .expect("retired procedure stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "review next steps".to_string(),
                intent: memvid_core::agent_memory::enums::QueryIntent::TaskState,
                entity: None,
                slot: Some("review".to_string()),
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
        Some(active.memory_id.as_str())
    );
    assert!(
        hits.iter()
            .all(|hit| hit.memory_id.as_deref() != Some(retired_id.as_str()))
    );
    assert!(
        hits.iter()
            .any(|hit| hit.memory_id.as_deref() == Some(cooling.memory_id.as_str()))
    );
}

#[test]
fn system_seeded_procedure_candidate_promotes_without_repetition() {
    let (mut controller, sink) = controller(ts(1_700_000_000));
    let seeded = CandidateMemory {
        candidate_id: "seeded-procedure".to_string(),
        observed_at: ts(1_700_000_000),
        entity: Some("procedure".to_string()),
        slot: Some("repo_review".to_string()),
        value: Some("repo_review".to_string()),
        raw_text: "Review the repo in a consistent order".to_string(),
        source: Provenance {
            source_type: SourceType::System,
            source_id: "system-seed".to_string(),
            source_label: Some("system".to_string()),
            observed_by: None,
            trust_weight: 1.0,
        },
        memory_type: MemoryType::Trace,
        confidence: 0.95,
        salience: 0.9,
        scope: Scope::Project,
        ttl: None,
        event_at: None,
        valid_from: None,
        valid_to: None,
        internal_layer: Some(MemoryLayer::Procedure),
        tags: vec!["review".to_string()],
        metadata: std::collections::BTreeMap::from([
            ("workflow_key".to_string(), "repo_review".to_string()),
            ("seeded_by_system".to_string(), "true".to_string()),
            ("procedure_name".to_string(), "repo_review".to_string()),
        ]),
        is_retraction: false,
    };

    let stored_id = controller
        .ingest(seeded)
        .expect("ingest succeeds")
        .expect("procedure stored");

    let procedure = {
        let mut procedure_store = ProcedureStore::new(controller.store_mut());
        procedure_store
            .get_by_workflow_key("repo_review")
            .expect("procedure lookup succeeds")
            .expect("procedure exists")
    };
    let promotion_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "promotion")
        .expect("promotion event exists");

    assert_eq!(stored_id, procedure.procedure_id);
    assert_eq!(procedure.success_count, 0);
    assert_eq!(procedure.status, ProcedureStatus::Active);
    assert_eq!(
        promotion_event
            .details
            .get("route_basis")
            .map(String::as_str),
        Some("system_seeded")
    );
}

#[test]
fn successful_workflow_promotion_persists_status_transition_metadata() {
    let mut store = InMemoryMemoryStore::default();
    store
        .put_memory(&procedure_memory(
            "procedure-stale",
            ProcedureStatus::CoolingDown,
            3,
            4,
            ts(1_700_000_000),
        ))
        .expect("stale procedure stored");

    let mut first = durable(
        "project",
        "event",
        "repo_review",
        "Completed the repo review workflow successfully",
        MemoryType::Episode,
        SourceType::Tool,
        0.8,
        ts(1_700_000_010),
    );
    first
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    first
        .metadata
        .insert("outcome".to_string(), "success".to_string());

    let mut second = durable(
        "project",
        "event",
        "repo_review",
        "Completed the repo review workflow successfully again",
        MemoryType::Episode,
        SourceType::Tool,
        0.8,
        ts(1_700_000_020),
    );
    second
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    second
        .metadata
        .insert("outcome".to_string(), "success".to_string());

    {
        let mut episode_store = EpisodeStore::new(&mut store);
        episode_store
            .save_memory(&first)
            .expect("first episode stored");
        episode_store
            .save_memory(&second)
            .expect("second episode stored");
    }

    let outcomes = ConsolidationEngine::default()
        .consolidate(
            &mut store,
            Some(&second),
            None,
            &FixedClock::new(ts(1_700_000_030)),
        )
        .expect("consolidation succeeds");

    let transition = outcomes
        .iter()
        .find_map(|outcome| outcome.procedure_status_transition.as_ref())
        .expect("procedure transition present");
    assert_eq!(transition.previous_status, ProcedureStatus::CoolingDown);
    assert_eq!(transition.next_status, ProcedureStatus::Active);

    let learned_procedure = {
        let mut procedure_store = ProcedureStore::new(&mut store);
        procedure_store
            .get_by_workflow_key("repo_review")
            .expect("procedure lookup succeeds")
            .expect("procedure exists")
    };
    assert_eq!(learned_procedure.status, ProcedureStatus::Active);
    assert_eq!(
        learned_procedure
            .metadata
            .get("prior_procedure_status")
            .map(String::as_str),
        Some("cooling_down")
    );
}

#[test]
fn failed_workflow_updates_existing_procedure_lifecycle() {
    let mut store = InMemoryMemoryStore::default();
    store
        .put_memory(&procedure_memory(
            "procedure-active-to-cooling",
            ProcedureStatus::Active,
            2,
            2,
            ts(1_700_000_000),
        ))
        .expect("existing procedure stored");

    let mut failed_episode = durable(
        "project",
        "event",
        "repo_review",
        "The repo review workflow failed due to missing approvals",
        MemoryType::Episode,
        SourceType::Tool,
        0.8,
        ts(1_700_000_010),
    );
    failed_episode
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    failed_episode
        .metadata
        .insert("outcome".to_string(), "failed".to_string());

    {
        let mut episode_store = EpisodeStore::new(&mut store);
        episode_store
            .save_memory(&failed_episode)
            .expect("failed episode stored");
    }

    let outcomes = ConsolidationEngine::default()
        .consolidate(
            &mut store,
            Some(&failed_episode),
            None,
            &FixedClock::new(ts(1_700_000_020)),
        )
        .expect("consolidation succeeds");

    let transition = outcomes
        .iter()
        .find_map(|outcome| outcome.procedure_status_transition.as_ref())
        .expect("failure transition present");
    assert_eq!(transition.previous_status, ProcedureStatus::Active);
    assert_eq!(transition.next_status, ProcedureStatus::CoolingDown);
    let failure_outcome = outcomes
        .iter()
        .find(|outcome| {
            outcome.record.metadata.get("outcome").map(String::as_str) == Some("failure")
        })
        .expect("failure consolidation recorded");
    assert_eq!(
        failure_outcome
            .record
            .metadata
            .get("outcome")
            .map(String::as_str),
        Some("failure")
    );

    let updated_procedure = {
        let mut procedure_store = ProcedureStore::new(&mut store);
        procedure_store
            .get_by_workflow_key("repo_review")
            .expect("procedure lookup succeeds")
            .expect("procedure exists")
    };
    assert_eq!(updated_procedure.status, ProcedureStatus::CoolingDown);
    assert_eq!(updated_procedure.failure_count, 3);
}

#[test]
fn controller_emits_procedure_status_change_when_reconciling_stale_lifecycle() {
    let (mut controller, sink) = controller(ts(1_700_000_000));
    controller
        .store_mut()
        .put_memory(&procedure_memory(
            "procedure-stale-active",
            ProcedureStatus::Active,
            1,
            6,
            ts(1_699_999_000),
        ))
        .expect("stale procedure stored");

    controller
        .ingest(candidate(
            "user",
            "location",
            "Berlin",
            "The user currently lives in Berlin.",
        ))
        .expect("ingest succeeds")
        .expect("memory stored");

    let transition_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "procedure_status_changed")
        .expect("status change event emitted");
    assert_eq!(
        transition_event
            .details
            .get("previous_status")
            .map(String::as_str),
        Some("active")
    );
    assert_eq!(
        transition_event
            .details
            .get("next_status")
            .map(String::as_str),
        Some("retired")
    );
    assert_eq!(
        transition_event
            .details
            .get("transition_reason")
            .map(String::as_str),
        Some("reconciliation")
    );

    let transition_trace = controller
        .store()
        .traces()
        .iter()
        .find(|(_, _, metadata)| {
            metadata.get("action").map(String::as_str) == Some("procedure_status_changed")
                && metadata.get("source").map(String::as_str) == Some("reconciliation")
        })
        .expect("persisted transition trace emitted");
    assert_eq!(
        transition_trace
            .2
            .get("previous_status")
            .map(String::as_str),
        Some("active")
    );
    assert_eq!(
        transition_trace.2.get("next_status").map(String::as_str),
        Some("retired")
    );
    assert_eq!(
        transition_trace
            .2
            .get("transition_reason")
            .map(String::as_str),
        Some("reconciliation")
    );
}

#[test]
fn controller_surfaces_failure_transition_reason_and_history_query() {
    let (mut controller, sink) = controller(ts(1_700_000_000));
    controller
        .store_mut()
        .put_memory(&procedure_memory(
            "procedure-active-to-cooling-controller",
            ProcedureStatus::Active,
            2,
            2,
            ts(1_699_999_900),
        ))
        .expect("existing procedure stored");

    let mut failed_candidate = candidate(
        "",
        "",
        "",
        "today the repo review failed due to missing approvals",
    );
    failed_candidate
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    failed_candidate
        .metadata
        .insert("outcome".to_string(), "failed".to_string());

    controller
        .ingest(failed_candidate)
        .expect("ingest succeeds")
        .expect("episode stored");

    let failure_event = sink
        .events()
        .into_iter()
        .find(|event| {
            event.action == "procedure_status_changed"
                && event.details.get("transition_reason").map(String::as_str) == Some("failure")
        })
        .expect("failure status change event emitted");
    assert_eq!(
        failure_event.details.get("source").map(String::as_str),
        Some("consolidation")
    );

    let history_hits = controller
        .retrieve(RetrievalQuery {
            query_text: "repo_review procedure lifecycle history".to_string(),
            intent: memvid_core::agent_memory::enums::QueryIntent::SemanticBackground,
            entity: Some("procedure".to_string()),
            slot: Some("repo_review".to_string()),
            scope: None,
            top_k: 3,
            as_of: None,
            include_expired: false,
        })
        .expect("history retrieval succeeds");

    let lifecycle_hit = history_hits
        .iter()
        .find(|hit| {
            hit.metadata.get("action").map(String::as_str) == Some("procedure_status_changed")
        })
        .expect("lifecycle trace hit returned");
    assert_eq!(lifecycle_hit.memory_layer, Some(MemoryLayer::Trace));
    assert_eq!(
        lifecycle_hit
            .metadata
            .get("transition_reason")
            .map(String::as_str),
        Some("failure")
    );
}

#[test]
fn newer_invalid_procedure_row_does_not_hide_valid_existing_workflow() {
    let mut store = InMemoryMemoryStore::default();
    let valid = procedure_memory(
        "procedure-valid-existing",
        ProcedureStatus::Active,
        3,
        0,
        ts(1_700_000_000),
    );
    store.put_memory(&valid).expect("valid procedure stored");

    let mut invalid = valid.clone();
    invalid.memory_id = "procedure-invalid-newer".to_string();
    invalid.stored_at = ts(1_700_000_100);
    invalid.slot = "   ".to_string();
    invalid.value = "   ".to_string();
    invalid
        .metadata
        .insert("workflow_key".to_string(), "   ".to_string());
    invalid
        .metadata
        .insert("procedure_name".to_string(), "   ".to_string());
    store
        .put_memory(&invalid)
        .expect("invalid procedure stored");

    let procedure = {
        let mut procedure_store = ProcedureStore::new(&mut store);
        procedure_store
            .get_by_workflow_key("repo_review")
            .expect("procedure lookup succeeds")
            .expect("procedure still exists")
    };

    assert_eq!(procedure.procedure_id, valid.memory_id);
    assert_eq!(procedure.status, ProcedureStatus::Active);
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
