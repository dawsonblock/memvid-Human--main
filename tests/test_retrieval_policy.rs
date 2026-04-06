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
    store
        .put_memory(&durable(
            "project",
            "task_status",
            "blocked",
            "The current task is blocked waiting on review",
            MemoryType::GoalState,
            SourceType::Chat,
            0.75,
            ts(1_700_000_050),
        ))
        .expect("goal stored");
    store
        .put_memory(&durable(
            "project",
            "event",
            "review_requested",
            "Yesterday the team requested review for the task",
            MemoryType::Episode,
            SourceType::Chat,
            0.75,
            ts(1_700_000_040),
        ))
        .expect("episode stored");
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
