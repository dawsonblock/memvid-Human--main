mod common;

use memvid_core::agent_memory::adapters::memvid_store::{InMemoryMemoryStore, MemoryStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::{
    BeliefStatus, MemoryLayer, MemoryType, QueryIntent, SourceType,
};
use memvid_core::agent_memory::memory_retriever::MemoryRetriever;
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::ranker::Ranker;
use memvid_core::agent_memory::retention::RetentionManager;
use memvid_core::agent_memory::schemas::{BeliefRecord, RetrievalHit, RetrievalQuery};

use common::{durable, ts};

fn score_components(hit: &RetrievalHit) -> std::collections::BTreeMap<String, f32> {
    hit.metadata
        .get("score_components")
        .expect("score components metadata present")
        .split('|')
        .filter_map(|entry| entry.split_once('='))
        .map(|(key, value)| {
            (
                key.to_string(),
                value.parse::<f32>().expect("score component parses"),
            )
        })
        .collect()
}

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
            contradictions_observed: 0,
            last_contradiction_at: None,
            time_to_last_resolution_seconds: None,
            positive_outcome_count: 0,
            negative_outcome_count: 0,
            last_outcome_at: None,
            source_weights: std::collections::BTreeMap::from([(SourceType::Chat, 0.75)]),
        })
        .expect("belief stored");
    let mut archived = durable(
        "user",
        "location",
        "Berlin",
        "Berlin appears in archive",
        MemoryType::Fact,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    archived.memory_id = "m1".to_string();
    archived.metadata.insert(
        "supporting_episode_ids".to_string(),
        "episode-1".to_string(),
    );
    store.put_memory(&archived).expect("memory stored");
    let mut support_episode = durable(
        "user",
        "location",
        "Berlin",
        "The system observed Berlin during profile sync",
        MemoryType::Episode,
        SourceType::Tool,
        0.9,
        ts(1_700_000_010),
    );
    support_episode.memory_id = "episode-1".to_string();
    store
        .put_memory(&support_episode)
        .expect("support episode stored");

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
    assert!(hits.iter().skip(1).any(|hit| {
        hit.metadata.get("retrieval_role").map(String::as_str) == Some("support_evidence")
    }));
    assert!(
        hits.iter()
            .any(|hit| hit.memory_layer == Some(MemoryLayer::Episode))
    );
}

#[test]
fn noisy_support_evidence_does_not_displace_direct_belief_answer() {
    let mut store = InMemoryMemoryStore::default();
    store
        .update_belief(&BeliefRecord {
            belief_id: "belief-support-heavy".to_string(),
            entity: "user".to_string(),
            slot: "location".to_string(),
            current_value: "Berlin".to_string(),
            status: BeliefStatus::Active,
            confidence: 0.95,
            valid_from: ts(1_700_000_000),
            valid_to: None,
            last_reviewed_at: ts(1_700_000_090),
            supporting_memory_ids: vec!["support-0".to_string()],
            opposing_memory_ids: Vec::new(),
            contradictions_observed: 0,
            last_contradiction_at: None,
            time_to_last_resolution_seconds: None,
            positive_outcome_count: 0,
            negative_outcome_count: 0,
            last_outcome_at: None,
            source_weights: std::collections::BTreeMap::from([(SourceType::Tool, 0.95)]),
        })
        .expect("belief stored");

    for index in 0..8 {
        let mut support = durable(
            "user",
            "location",
            "Berlin",
            &format!("Support evidence {index} still points to Berlin"),
            if index % 2 == 0 {
                MemoryType::Episode
            } else {
                MemoryType::Fact
            },
            SourceType::Tool,
            0.95,
            ts(1_700_000_001 + index),
        );
        support.memory_id = format!("support-{index}");
        store.put_memory(&support).expect("support stored");
    }

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
                top_k: 8,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_200)),
        )
        .expect("retrieval works");

    assert!(hits.first().expect("hit").from_belief);
    assert_eq!(
        hits.first()
            .and_then(|hit| hit.metadata.get("retrieval_role"))
            .map(String::as_str),
        Some("direct_answer")
    );
    assert!(hits.iter().skip(1).all(|hit| !hit.from_belief));
}

#[test]
fn historical_fact_queries_prefer_episodes_and_prior_state_evidence() {
    let mut store = InMemoryMemoryStore::default();
    let mut old_memory = durable(
        "user",
        "location",
        "Berlin",
        "Earlier records show Berlin",
        MemoryType::Fact,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    old_memory.event_at = Some(ts(1_700_000_000));
    old_memory.source.source_id = "archive-source".to_string();
    store.put_memory(&old_memory).expect("old state stored");

    let mut historical_episode = durable(
        "user",
        "location",
        "Berlin",
        "Yesterday the user said they were in Berlin",
        MemoryType::Episode,
        SourceType::Chat,
        0.75,
        ts(1_700_000_050),
    );
    historical_episode.event_at = Some(ts(1_700_000_050));
    store
        .put_memory(&historical_episode)
        .expect("historical episode stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "where was the user before".to_string(),
                intent: QueryIntent::HistoricalFact,
                entity: Some("user".to_string()),
                slot: Some("location".to_string()),
                scope: None,
                top_k: 4,
                as_of: Some(ts(1_700_000_100)),
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_200)),
        )
        .expect("historical retrieval works");

    assert_eq!(
        hits.first().and_then(|hit| hit.memory_layer),
        Some(MemoryLayer::Episode)
    );
    assert!(
        hits.iter()
            .any(|hit| hit.memory_layer == Some(MemoryLayer::Belief))
    );
}

#[test]
fn disputed_belief_surfaces_as_contested_current_fact_when_no_active_belief_exists() {
    let mut store = InMemoryMemoryStore::default();
    store
        .update_belief(&BeliefRecord {
            belief_id: "belief-disputed".to_string(),
            entity: "user".to_string(),
            slot: "location".to_string(),
            current_value: "Berlin".to_string(),
            status: BeliefStatus::Disputed,
            confidence: 0.88,
            valid_from: ts(1_700_000_000),
            valid_to: None,
            last_reviewed_at: ts(1_700_000_120),
            supporting_memory_ids: vec!["support-berlin".to_string()],
            opposing_memory_ids: vec!["support-paris".to_string()],
            contradictions_observed: 2,
            last_contradiction_at: Some(ts(1_700_000_100)),
            time_to_last_resolution_seconds: None,
            positive_outcome_count: 0,
            negative_outcome_count: 0,
            last_outcome_at: None,
            source_weights: std::collections::BTreeMap::from([(SourceType::Tool, 0.95)]),
        })
        .expect("belief stored");
    store
        .put_memory(&durable(
            "user",
            "location",
            "Berlin",
            "A trusted tool still points to Berlin",
            MemoryType::Fact,
            SourceType::Tool,
            0.95,
            ts(1_700_000_050),
        ))
        .expect("support memory stored");
    let mut opposing = durable(
        "user",
        "location",
        "Paris",
        "A new conflicting source points to Paris",
        MemoryType::Fact,
        SourceType::Chat,
        0.7,
        ts(1_700_000_100),
    );
    opposing.memory_id = "support-paris".to_string();
    store.put_memory(&opposing).expect("opposing memory stored");

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
                top_k: 4,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_200)),
        )
        .expect("retrieval works");

    let direct = hits.first().expect("contested direct hit");
    assert!(direct.from_belief);
    assert_eq!(
        direct.metadata.get("belief_status").map(String::as_str),
        Some("disputed")
    );
    assert_eq!(
        direct
            .metadata
            .get("belief_retrieval_status")
            .map(String::as_str),
        Some("contested")
    );
    assert_eq!(
        direct
            .metadata
            .get("contradictions_observed")
            .map(String::as_str),
        Some("2")
    );
    assert_eq!(
        direct
            .metadata
            .get("time_in_dispute_seconds")
            .map(String::as_str),
        Some("100")
    );
    assert!(hits.iter().skip(1).any(|hit| {
        hit.metadata.get("belief_relation").map(String::as_str) == Some("opposing")
    }));
}

#[test]
fn score_breakdown_in_metadata_contains_all_named_factors() {
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
            contradictions_observed: 0,
            last_contradiction_at: None,
            time_to_last_resolution_seconds: None,
            positive_outcome_count: 0,
            negative_outcome_count: 0,
            last_outcome_at: None,
            source_weights: std::collections::BTreeMap::from([(SourceType::Tool, 0.95)]),
        })
        .expect("belief stored");

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
                top_k: 1,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_200)),
        )
        .expect("retrieval works");

    let direct = hits.first().expect("direct belief hit");
    let components = score_components(direct);
    for key in [
        "layer_match",
        "role_priority",
        "belief_priority",
        "content_match",
        "goal_relevance",
        "self_relevance",
        "salience",
        "evidence_strength",
        "contradiction_penalty",
        "recency",
        "procedure_success",
        "goal_status_priority",
        "lifecycle_history_bonus",
        "procedure_lifecycle_penalty",
        "expiry_penalty",
        "total",
    ] {
        assert!(
            components.contains_key(key),
            "missing score component {key}"
        );
    }
    assert!(direct.metadata.contains_key("ranking_explanation"));
}

#[test]
fn score_breakdown_sums_to_final_score() {
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
                top_k: 1,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_100)),
        )
        .expect("retrieval works");

    let direct = hits.first().expect("direct self-model hit");
    let components = score_components(direct);
    let summed = components
        .iter()
        .filter(|(key, _)| key.as_str() != "total")
        .map(|(_, value)| *value)
        .sum::<f32>();
    let reported_total = components.get("total").copied().expect("total present");

    assert!((summed - direct.score).abs() < 0.0001);
    assert!((reported_total - direct.score).abs() < 0.0001);
}

#[test]
fn score_components_are_stable_across_runs() {
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

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let query = RetrievalQuery {
        query_text: "what editor does the user prefer".to_string(),
        intent: QueryIntent::PreferenceLookup,
        entity: Some("user".to_string()),
        slot: None,
        scope: None,
        top_k: 1,
        as_of: None,
        include_expired: false,
    };

    let first = retriever
        .retrieve(&mut store, &query, &FixedClock::new(ts(1_700_000_100)))
        .expect("first retrieval works");
    let second = retriever
        .retrieve(&mut store, &query, &FixedClock::new(ts(1_700_000_100)))
        .expect("second retrieval works");

    assert_eq!(
        first[0].metadata.get("score_components"),
        second[0].metadata.get("score_components")
    );
    assert_eq!(
        first[0].metadata.get("ranking_explanation"),
        second[0].metadata.get("ranking_explanation")
    );
    assert_eq!(first[0].score, second[0].score);
}

#[test]
fn ranking_explanation_documents_why_answer_is_primary() {
    let mut store = InMemoryMemoryStore::default();
    store
        .update_belief(&BeliefRecord {
            belief_id: "belief-why".to_string(),
            entity: "user".to_string(),
            slot: "location".to_string(),
            current_value: "Berlin".to_string(),
            status: BeliefStatus::Active,
            confidence: 0.92,
            valid_from: ts(1_700_000_000),
            valid_to: None,
            last_reviewed_at: ts(1_700_000_050),
            supporting_memory_ids: vec!["m-why".to_string()],
            opposing_memory_ids: Vec::new(),
            contradictions_observed: 0,
            last_contradiction_at: None,
            time_to_last_resolution_seconds: None,
            positive_outcome_count: 0,
            negative_outcome_count: 0,
            last_outcome_at: None,
            source_weights: std::collections::BTreeMap::from([(SourceType::Tool, 0.95)]),
        })
        .expect("belief stored");

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
                top_k: 1,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_100)),
        )
        .expect("retrieval works");

    let explanation = hits[0]
        .metadata
        .get("ranking_explanation")
        .expect("ranking explanation present");
    assert!(explanation.contains("direct_answer"));
    assert!(explanation.contains("belief"));
    assert!(explanation.contains("current_fact"));
}

#[test]
fn positive_belief_feedback_increases_current_fact_evidence_strength() {
    let mut positive_store = InMemoryMemoryStore::default();
    positive_store
        .update_belief(&BeliefRecord {
            belief_id: "belief-positive".to_string(),
            entity: "user".to_string(),
            slot: "location".to_string(),
            current_value: "Berlin".to_string(),
            status: BeliefStatus::Active,
            confidence: 0.8,
            valid_from: ts(1_700_000_000),
            valid_to: None,
            last_reviewed_at: ts(1_700_000_050),
            supporting_memory_ids: vec!["m-positive".to_string()],
            opposing_memory_ids: Vec::new(),
            contradictions_observed: 0,
            last_contradiction_at: None,
            time_to_last_resolution_seconds: None,
            positive_outcome_count: 3,
            negative_outcome_count: 0,
            last_outcome_at: Some(ts(1_700_000_090)),
            source_weights: std::collections::BTreeMap::from([(SourceType::Tool, 0.95)]),
        })
        .expect("positive belief stored");

    let mut negative_store = InMemoryMemoryStore::default();
    negative_store
        .update_belief(&BeliefRecord {
            belief_id: "belief-negative".to_string(),
            entity: "user".to_string(),
            slot: "location".to_string(),
            current_value: "Berlin".to_string(),
            status: BeliefStatus::Active,
            confidence: 0.8,
            valid_from: ts(1_700_000_000),
            valid_to: None,
            last_reviewed_at: ts(1_700_000_050),
            supporting_memory_ids: vec!["m-negative".to_string()],
            opposing_memory_ids: Vec::new(),
            contradictions_observed: 0,
            last_contradiction_at: None,
            time_to_last_resolution_seconds: None,
            positive_outcome_count: 0,
            negative_outcome_count: 3,
            last_outcome_at: Some(ts(1_700_000_090)),
            source_weights: std::collections::BTreeMap::from([(SourceType::Tool, 0.95)]),
        })
        .expect("negative belief stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let query = RetrievalQuery {
        query_text: "what is the user's current location".to_string(),
        intent: QueryIntent::CurrentFact,
        entity: Some("user".to_string()),
        slot: Some("location".to_string()),
        scope: None,
        top_k: 1,
        as_of: None,
        include_expired: false,
    };

    let positive_hits = retriever
        .retrieve(
            &mut positive_store,
            &query,
            &FixedClock::new(ts(1_700_000_100)),
        )
        .expect("positive retrieval works");
    let negative_hits = retriever
        .retrieve(
            &mut negative_store,
            &query,
            &FixedClock::new(ts(1_700_000_100)),
        )
        .expect("negative retrieval works");

    let positive_strength = positive_hits[0]
        .metadata
        .get("score_signal_evidence_strength")
        .and_then(|value| value.parse::<f32>().ok())
        .expect("positive evidence strength");
    let negative_strength = negative_hits[0]
        .metadata
        .get("score_signal_evidence_strength")
        .and_then(|value| value.parse::<f32>().ok())
        .expect("negative evidence strength");

    assert!(positive_strength > 0.8);
    assert!(negative_strength < 0.8);
    assert!(positive_hits[0].score > negative_hits[0].score);
}

#[test]
fn procedure_lifecycle_penalty_appears_in_breakdown_for_cooling_down_procedure() {
    let mut store = InMemoryMemoryStore::default();
    let mut procedure = durable(
        "procedure",
        "repo_review",
        "repo_review",
        "Review the repo in a consistent order.",
        MemoryType::Trace,
        SourceType::System,
        1.0,
        ts(1_700_000_000),
    );
    procedure.internal_layer = Some(MemoryLayer::Procedure);
    procedure
        .metadata
        .insert("procedure_name".to_string(), "repo_review".to_string());
    procedure
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    procedure
        .metadata
        .insert("context_tags".to_string(), "repo_review,review".to_string());
    procedure
        .metadata
        .insert("success_count".to_string(), "1".to_string());
    procedure
        .metadata
        .insert("failure_count".to_string(), "3".to_string());
    procedure
        .metadata
        .insert("procedure_status".to_string(), "cooling_down".to_string());
    store.put_memory(&procedure).expect("procedure stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "how do I run the repo_review workflow".to_string(),
                intent: QueryIntent::SemanticBackground,
                entity: None,
                slot: None,
                scope: None,
                top_k: 3,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_100)),
        )
        .expect("retrieval works");

    let direct = hits.first().expect("procedure hit");
    assert_eq!(direct.memory_layer, Some(MemoryLayer::Procedure));
    assert_eq!(
        direct
            .metadata
            .get("score_component_procedure_lifecycle_penalty")
            .map(String::as_str),
        Some("-1.500000")
    );
    assert!(
        direct
            .metadata
            .get("score_components")
            .expect("score components present")
            .contains("procedure_lifecycle_penalty=-1.500000")
    );
}

#[test]
fn recently_accessed_memory_ranks_above_equally_relevant_peer() {
    let mut store = InMemoryMemoryStore::default();
    let mut accessed = durable(
        "project",
        "note_accessed",
        "build",
        "Rust build checklist",
        MemoryType::Episode,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    accessed.memory_id = "accessed-note".to_string();
    accessed.source.source_id = "source-accessed".to_string();
    accessed
        .metadata
        .insert("retrieval_count".to_string(), "4".to_string());
    accessed.metadata.insert(
        "last_accessed_at".to_string(),
        ts(1_700_000_090).to_rfc3339(),
    );
    let mut baseline = durable(
        "project",
        "note_baseline",
        "build",
        "Rust build checklist",
        MemoryType::Episode,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    baseline.memory_id = "baseline-note".to_string();
    baseline.source.source_id = "source-baseline".to_string();

    store.put_memory(&baseline).expect("baseline stored");
    store.put_memory(&accessed).expect("accessed stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "rust build checklist".to_string(),
                intent: QueryIntent::SemanticBackground,
                entity: None,
                slot: None,
                scope: None,
                top_k: 2,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_100)),
        )
        .expect("retrieval works");

    assert_eq!(
        hits.first().and_then(|hit| hit.memory_id.as_deref()),
        Some("accessed-note")
    );
    assert_eq!(
        hits.first()
            .and_then(|hit| hit.metadata.get("retrieval_count"))
            .map(String::as_str),
        Some("4")
    );
}

#[test]
fn positive_outcome_feedback_ranks_memory_above_negative_peer() {
    let mut store = InMemoryMemoryStore::default();
    let mut positive = durable(
        "project",
        "review_note_positive",
        "review",
        "Repo review checklist",
        MemoryType::Episode,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    positive.memory_id = "positive-review-note".to_string();
    positive.source.source_id = "source-positive-review".to_string();
    positive
        .metadata
        .insert("positive_outcome_count".to_string(), "3".to_string());
    positive.metadata.insert(
        "last_outcome_at".to_string(),
        ts(1_700_000_090).to_rfc3339(),
    );

    let mut negative = durable(
        "project",
        "review_note_negative",
        "review",
        "Repo review checklist",
        MemoryType::Episode,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    negative.memory_id = "negative-review-note".to_string();
    negative.source.source_id = "source-negative-review".to_string();
    negative
        .metadata
        .insert("negative_outcome_count".to_string(), "3".to_string());
    negative.metadata.insert(
        "last_outcome_at".to_string(),
        ts(1_700_000_090).to_rfc3339(),
    );

    store.put_memory(&negative).expect("negative note stored");
    store.put_memory(&positive).expect("positive note stored");

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "repo review checklist".to_string(),
                intent: QueryIntent::SemanticBackground,
                entity: None,
                slot: None,
                scope: None,
                top_k: 2,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_100)),
        )
        .expect("retrieval works");

    assert_eq!(
        hits.first().and_then(|hit| hit.memory_id.as_deref()),
        Some("positive-review-note")
    );
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
fn blank_semantic_query_returns_no_hits_without_error() {
    let mut store = InMemoryMemoryStore::default();
    store
        .put_memory(&durable(
            "user",
            "location",
            "Berlin",
            "The user currently lives in Berlin",
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
                query_text: "   ".to_string(),
                intent: QueryIntent::SemanticBackground,
                entity: None,
                slot: None,
                scope: None,
                top_k: 3,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_100)),
        )
        .expect("retrieval succeeds");

    assert!(hits.is_empty());
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
fn preference_query_returns_self_model_with_limited_support() {
    let mut store = InMemoryMemoryStore::default();
    let mut first = durable(
        "user",
        "response_style",
        "concise",
        "The user prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    first.metadata.insert(
        "supporting_episode_ids".to_string(),
        "episode-1".to_string(),
    );
    store.put_memory(&first).expect("first preference stored");
    let second = durable(
        "user",
        "response_style",
        "concise",
        "The user again prefers concise responses",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_050),
    );
    store.put_memory(&second).expect("second preference stored");
    let mut support_episode = durable(
        "user",
        "event",
        "preference_confirmed",
        "A recent episode confirmed the concise response style",
        MemoryType::Episode,
        SourceType::Tool,
        0.9,
        ts(1_700_000_040),
    );
    support_episode.memory_id = "episode-1".to_string();
    store
        .put_memory(&support_episode)
        .expect("support episode stored");

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
                top_k: 4,
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
    assert!(hits.iter().skip(1).any(|hit| {
        hit.metadata.get("retrieval_role").map(String::as_str) == Some("support_evidence")
    }));
    assert!(hits.len() <= 4);
}

#[test]
fn procedural_help_query_returns_direct_procedure_with_bounded_support() {
    let mut store = InMemoryMemoryStore::default();
    let mut procedure = durable(
        "procedure",
        "repo_review",
        "repo_review",
        "Review the repo in a consistent order.",
        MemoryType::Trace,
        SourceType::System,
        1.0,
        ts(1_700_000_000),
    );
    procedure.internal_layer = Some(MemoryLayer::Procedure);
    procedure
        .metadata
        .insert("procedure_name".to_string(), "repo_review".to_string());
    procedure
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    procedure
        .metadata
        .insert("context_tags".to_string(), "repo_review,review".to_string());
    procedure
        .metadata
        .insert("success_count".to_string(), "4".to_string());
    procedure
        .metadata
        .insert("failure_count".to_string(), "0".to_string());
    procedure
        .metadata
        .insert("procedure_status".to_string(), "active".to_string());
    store.put_memory(&procedure).expect("procedure stored");

    for offset in 0..4 {
        let mut episode = durable(
            "project",
            "event",
            "repo_review",
            "Completed the repo review workflow successfully.",
            MemoryType::Episode,
            SourceType::Tool,
            0.9,
            ts(1_700_000_010 + offset),
        );
        episode
            .metadata
            .insert("workflow_key".to_string(), "repo_review".to_string());
        episode
            .metadata
            .insert("outcome".to_string(), "success".to_string());
        episode.tags.push("review".to_string());
        store.put_memory(&episode).expect("support episode stored");
    }

    let retriever = MemoryRetriever::new(Ranker, RetentionManager::new(PolicySet::default()));
    let hits = retriever
        .retrieve(
            &mut store,
            &RetrievalQuery {
                query_text: "how do I run the repo_review workflow".to_string(),
                intent: QueryIntent::SemanticBackground,
                entity: None,
                slot: None,
                scope: None,
                top_k: 5,
                as_of: None,
                include_expired: false,
            },
            &FixedClock::new(ts(1_700_000_100)),
        )
        .expect("retrieval works");

    assert_eq!(
        hits.first().and_then(|hit| hit.memory_layer),
        Some(MemoryLayer::Procedure)
    );
    assert_eq!(
        hits.first()
            .and_then(|hit| hit.metadata.get("retrieval_role").map(String::as_str)),
        Some("direct_answer")
    );

    let support_hits: Vec<_> = hits
        .iter()
        .filter(|hit| {
            hit.metadata.get("retrieval_role").map(String::as_str) == Some("support_evidence")
        })
        .collect();
    assert!(!support_hits.is_empty());
    assert!(support_hits.len() <= 3);
    assert!(
        support_hits
            .iter()
            .all(|hit| hit.memory_layer == Some(MemoryLayer::Episode))
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
    let mut duplicate_episode = durable(
        "project",
        "event",
        "review_requested",
        "The team requested review for the task",
        MemoryType::Episode,
        SourceType::Chat,
        0.75,
        ts(1_700_000_005),
    );
    duplicate_episode.memory_id = "memory-project-event-review_requested-duplicate".to_string();
    store
        .put_memory(&duplicate_episode)
        .expect("duplicate episode stored");

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

    let duplicate_hits: Vec<_> = hits
        .iter()
        .filter(|hit| hit.value.as_deref() == Some("review_requested"))
        .collect();
    assert_eq!(duplicate_hits.len(), 1);
}
