mod common;

use memvid_core::agent_memory::adapters::memvid_store::{InMemoryMemoryStore, MemoryStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::{BeliefStatus, MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::memory_retriever::MemoryRetriever;
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::ranker::Ranker;
use memvid_core::agent_memory::retention::RetentionManager;
use memvid_core::agent_memory::schemas::{BeliefRecord, RetrievalQuery};

use common::{durable, ts};

#[test]
fn current_fact_query_checks_belief_state_first() {
    let mut store = InMemoryMemoryStore::default();
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
            last_reviewed_at: ts(1_700_000_100),
            supporting_memory_ids: vec!["m1".to_string()],
            opposing_memory_ids: Vec::new(),
            source_weights: std::collections::BTreeMap::from([(SourceType::Chat, 0.75)]),
        })
        .expect("belief stored");
    store
        .put_memory(&durable(
            "user",
            "location",
            "Berlin",
            "Berlin appears in archive",
            MemoryType::Fact,
            SourceType::Chat,
            0.75,
            ts(1_700_000_000),
        ))
        .expect("memory stored");

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
            &FixedClock::new(ts(1_700_000_200)),
        )
        .expect("retrieval works");

    assert!(hits.first().expect("hit").from_belief);
}

#[test]
fn preference_query_ranks_preference_memory_above_generic_semantic_hits() {
    let mut store = InMemoryMemoryStore::default();
    store
        .put_memory(&durable(
            "user",
            "favorite_editor",
            "vim",
            "The user prefers vim for editing",
            MemoryType::Preference,
            SourceType::Chat,
            0.75,
            ts(1_700_000_000),
        ))
        .expect("preference stored");
    store
        .put_memory(&durable(
            "user",
            "bio",
            "writes code",
            "The user writes code in many editors including vim and emacs",
            MemoryType::Fact,
            SourceType::Chat,
            0.75,
            ts(1_700_000_000),
        ))
        .expect("background stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "what editor does the user prefer".to_string(),
                intent: QueryIntent::PreferenceLookup,
                entity: Some("user".to_string()),
                slot: None,
                scope: None,
                top_k: 3,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_100)),
        )
        .expect("retrieval works");

    assert_eq!(
        hits.first().and_then(|hit| hit.memory_type),
        Some(MemoryType::Preference)
    );
}

#[test]
fn task_query_ranks_goal_state_and_recent_episodes_above_background_text() {
    let mut store = InMemoryMemoryStore::default();
    let episode = durable(
        "project",
        "event",
        "review_requested",
        "Yesterday the team requested review for the task",
        MemoryType::Episode,
        SourceType::Chat,
        0.75,
        ts(1_700_000_040),
    );
    let episode_id = episode.memory_id.clone();
    store.put_memory(&episode).expect("episode stored");

    let mut goal = durable(
        "project",
        "task_status",
        "blocked",
        "The current task is blocked waiting on review",
        MemoryType::GoalState,
        SourceType::Chat,
        0.75,
        ts(1_700_000_050),
    );
    goal.metadata
        .insert("supporting_episode_ids".to_string(), episode_id);
    store.put_memory(&goal).expect("goal stored");
    store
        .put_memory(&durable(
            "project",
            "summary",
            "documentation",
            "Background documentation mentions the task and the review process",
            MemoryType::Fact,
            SourceType::Chat,
            0.75,
            ts(1_699_000_000),
        ))
        .expect("background stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "what is the current task status".to_string(),
                intent: QueryIntent::TaskState,
                entity: Some("project".to_string()),
                slot: None,
                scope: None,
                top_k: 3,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_100)),
        )
        .expect("retrieval works");

    assert_eq!(
        hits.first().and_then(|hit| hit.memory_type),
        Some(MemoryType::GoalState)
    );
    assert_eq!(
        hits.get(1).and_then(|hit| hit.memory_type),
        Some(MemoryType::Episode)
    );
}

#[test]
fn task_query_excludes_unaligned_episode_and_procedure_context() {
    let mut store = InMemoryMemoryStore::default();
    let supporting_episode = durable(
        "project",
        "event",
        "review_requested",
        "Requested review for the current task",
        MemoryType::Episode,
        SourceType::Chat,
        0.75,
        ts(1_700_000_010),
    );
    let supporting_episode_id = supporting_episode.memory_id.clone();
    store
        .put_memory(&supporting_episode)
        .expect("supporting episode stored");
    store
        .put_memory(&durable(
            "project",
            "event",
            "unrelated_followup",
            "A different thread discussed documentation cleanup",
            MemoryType::Episode,
            SourceType::Chat,
            0.75,
            ts(1_700_000_020),
        ))
        .expect("unrelated episode stored");

    let mut goal = durable(
        "project",
        "task_status",
        "blocked",
        "Blocked waiting on review",
        MemoryType::GoalState,
        SourceType::Chat,
        0.75,
        ts(1_700_000_030),
    );
    goal.metadata
        .insert("supporting_episode_ids".to_string(), supporting_episode_id);
    goal.metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    store.put_memory(&goal).expect("goal stored");

    let mut aligned_procedure = durable(
        "procedure",
        "repo_review",
        "repo_review",
        "Review the repo in a consistent order",
        MemoryType::Trace,
        SourceType::System,
        1.0,
        ts(1_700_000_040),
    );
    aligned_procedure.internal_layer =
        Some(memvid_core::agent_memory::enums::MemoryLayer::Procedure);
    aligned_procedure
        .metadata
        .insert("procedure_name".to_string(), "repo_review".to_string());
    aligned_procedure
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    aligned_procedure
        .metadata
        .insert("context_tags".to_string(), "repo_review,review".to_string());
    aligned_procedure
        .metadata
        .insert("procedure_status".to_string(), "active".to_string());
    store
        .put_memory(&aligned_procedure)
        .expect("aligned procedure stored");

    let mut unrelated_procedure = aligned_procedure.clone();
    unrelated_procedure.memory_id = "memory-procedure-unrelated".to_string();
    unrelated_procedure.slot = "doc_cleanup".to_string();
    unrelated_procedure.value = "doc_cleanup".to_string();
    unrelated_procedure
        .metadata
        .insert("workflow_key".to_string(), "doc_cleanup".to_string());
    unrelated_procedure
        .metadata
        .insert("context_tags".to_string(), "docs,cleanup".to_string());
    store
        .put_memory(&unrelated_procedure)
        .expect("unrelated procedure stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "what is the current task status for repo_review".to_string(),
                intent: QueryIntent::TaskState,
                entity: Some("project".to_string()),
                slot: None,
                scope: None,
                top_k: 5,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_100)),
        )
        .expect("retrieval works");

    assert!(
        hits.iter()
            .any(|hit| hit.memory_id.as_deref() == Some(aligned_procedure.memory_id.as_str()))
    );
    assert!(
        hits.iter()
            .all(|hit| hit.memory_id.as_deref() != Some("memory-procedure-unrelated"))
    );
    assert!(
        hits.iter()
            .all(|hit| hit.value.as_deref() != Some("unrelated_followup"))
    );
}

#[test]
fn preference_query_uses_direct_self_model_lookup_when_text_overlap_is_weak() {
    let mut store = InMemoryMemoryStore::default();
    store
        .put_memory(&durable(
            "user",
            "response_style",
            "concise",
            "Favor terse replies.",
            MemoryType::Preference,
            SourceType::Chat,
            0.75,
            ts(1_700_000_000),
        ))
        .expect("preference stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "communication guidance".to_string(),
                intent: QueryIntent::PreferenceLookup,
                entity: Some("user".to_string()),
                slot: None,
                scope: None,
                top_k: 3,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_100)),
        )
        .expect("retrieval works");

    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits.first().and_then(|hit| hit.memory_type),
        Some(MemoryType::Preference)
    );
}

#[test]
fn preference_query_falls_back_to_search_when_self_model_store_is_empty() {
    let mut store = InMemoryMemoryStore::default();
    store
        .put_memory(&durable(
            "user",
            "notes",
            "editor_history",
            "The project notes say the user prefers vim during reviews",
            MemoryType::Fact,
            SourceType::Chat,
            0.75,
            ts(1_700_000_000),
        ))
        .expect("fact stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "what editor does the user prefer".to_string(),
                intent: QueryIntent::PreferenceLookup,
                entity: Some("user".to_string()),
                slot: None,
                scope: None,
                top_k: 3,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_050)),
        )
        .expect("retrieval works");

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].memory_type, Some(MemoryType::Fact));
}

#[test]
fn task_query_uses_goal_state_store_when_text_overlap_is_weak() {
    let mut store = InMemoryMemoryStore::default();
    store
        .put_memory(&durable(
            "project",
            "task_status",
            "blocked",
            "Waiting on system dependency before continuing",
            MemoryType::GoalState,
            SourceType::Chat,
            0.75,
            ts(1_700_000_000),
        ))
        .expect("goal stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "where should execution resume".to_string(),
                intent: QueryIntent::TaskState,
                entity: Some("project".to_string()),
                slot: None,
                scope: None,
                top_k: 3,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_100)),
        )
        .expect("retrieval works");

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].memory_type, Some(MemoryType::GoalState));
}

#[test]
fn task_query_deduplicates_supporting_and_recent_episode_hits() {
    let mut store = InMemoryMemoryStore::default();
    let episode = durable(
        "project",
        "event",
        "review_requested",
        "The team requested review for the task",
        MemoryType::Episode,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    store.put_memory(&episode).expect("episode stored");

    let mut goal = durable(
        "project",
        "task_status",
        "blocked",
        "Blocked waiting on review",
        MemoryType::GoalState,
        SourceType::Chat,
        0.75,
        ts(1_700_000_010),
    );
    goal.metadata.insert(
        "supporting_episode_ids".to_string(),
        episode.memory_id.clone(),
    );
    store.put_memory(&goal).expect("goal stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "what is the current task status".to_string(),
                intent: QueryIntent::TaskState,
                entity: Some("project".to_string()),
                slot: None,
                scope: None,
                top_k: 5,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_020)),
        )
        .expect("retrieval works");

    let unique_ids: std::collections::HashSet<_> = hits
        .iter()
        .filter_map(|hit| hit.memory_id.as_deref())
        .collect();
    assert_eq!(unique_ids.len(), hits.len());
}
