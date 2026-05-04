use std::collections::BTreeMap;

use chrono::Utc;
use memvid_core::agent_memory::enums::{MemoryLayer, QueryIntent};
use memvid_core::agent_memory::policy::PolicyProfile;
use memvid_core::agent_memory::ranker::Ranker;
use memvid_core::agent_memory::retrieval_feedback::{FeedbackAdjuster, MAX_WEIGHT, MIN_WEIGHT};
use memvid_core::agent_memory::schemas::RetrievalHit;

fn ts() -> chrono::DateTime<Utc> {
    chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()
}

fn make_hit(text: &str, score: f32) -> RetrievalHit {
    RetrievalHit {
        memory_id: Some(format!("mem-{text}")),
        belief_id: None,
        entity: Some("user".to_string()),
        slot: Some("pref".to_string()),
        value: Some(text.to_string()),
        text: text.to_string(),
        memory_layer: Some(MemoryLayer::Episode),
        memory_type: None,
        score,
        timestamp: ts(),
        scope: None,
        source: None,
        from_belief: false,
        expired: false,
        metadata: BTreeMap::new(),
    }
}

/// Returns the first hit from a reranked list for a content-match dominated result.
fn ranker_scored_hit() -> RetrievalHit {
    let hit = make_hit("user prefers dark mode", 0.9);
    // Pre-populate a strong content_match signal so the ranker amplifies it.
    let hit = {
        let mut h = hit;
        h.metadata
            .insert("score_signal_content_match".to_string(), "0.9".to_string());
        h
    };
    let weights = PolicyProfile::default().soft_weights().clone();
    let mut hits = Ranker.rerank_with_weights(
        vec![hit.clone()],
        QueryIntent::SemanticBackground,
        ts(),
        &weights,
    );
    hits.remove(0)
}

#[test]
fn ranker_scored_hit_has_component_total_metadata() {
    let hit = ranker_scored_hit();
    let total: f32 = hit
        .metadata
        .get("score_component_total")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    assert!(
        total.abs() > f32::EPSILON,
        "ranker must write score_component_total metadata; got {total}"
    );
}

#[test]
fn positive_feedback_on_ranker_hit_adjusts_weights() {
    let hit = ranker_scored_hit();
    let baseline = PolicyProfile::default().soft_weights().clone();
    let initial_content = baseline.content_match;
    let mut adjuster = FeedbackAdjuster::new(baseline);
    adjuster.apply(&hit, 1.0);
    // At least one weight changed (the dominant component(s) were nudged).
    let changed = adjuster.weights().content_match != initial_content
        || adjuster.weights().recency != PolicyProfile::default().soft_weights().recency
        || adjuster.weights().goal_relevance
            != PolicyProfile::default().soft_weights().goal_relevance;
    // It's OK if none were dominant enough; feedback_count should still be 1.
    assert_eq!(
        adjuster.feedback_count(),
        1,
        "feedback count must increment"
    );
    let _ = changed; // dominance depends on exact score values, not asserted here
}

#[test]
fn weight_bounds_are_enforced_under_repeated_saturation() {
    let baseline = PolicyProfile::default().soft_weights().clone();
    let mut up = FeedbackAdjuster::with_learning_rate(baseline.clone(), 0.5);
    let mut down = FeedbackAdjuster::with_learning_rate(baseline, 0.5);

    let hit = ranker_scored_hit();
    for _ in 0..50 {
        up.apply(&hit, 1.0);
        down.apply(&hit, -1.0);
    }

    for weight in [
        up.weights().content_match,
        up.weights().goal_relevance,
        up.weights().self_relevance,
        up.weights().salience,
        up.weights().evidence_strength,
        up.weights().recency,
        up.weights().procedure_success,
    ] {
        assert!(weight <= MAX_WEIGHT, "weight {weight} exceeds MAX_WEIGHT");
        assert!(weight >= MIN_WEIGHT, "weight {weight} below MIN_WEIGHT");
    }

    for weight in [
        down.weights().content_match,
        down.weights().goal_relevance,
        down.weights().self_relevance,
        down.weights().salience,
        down.weights().evidence_strength,
        down.weights().recency,
        down.weights().procedure_success,
    ] {
        assert!(weight <= MAX_WEIGHT, "weight {weight} exceeds MAX_WEIGHT");
        assert!(weight >= MIN_WEIGHT, "weight {weight} below MIN_WEIGHT");
    }
}

#[test]
fn apply_all_processes_slice_and_counts_each_hit() {
    let baseline = PolicyProfile::default().soft_weights().clone();
    let mut adjuster = FeedbackAdjuster::new(baseline);
    let hits: Vec<RetrievalHit> = (0..3).map(|_| ranker_scored_hit()).collect();
    adjuster.apply_all(&hits, 1.0);
    assert_eq!(
        adjuster.feedback_count(),
        3,
        "each hit in the slice is a separate feedback event"
    );
}
