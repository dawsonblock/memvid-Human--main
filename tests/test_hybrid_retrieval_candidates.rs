mod common;

use common::{durable, ts};
use memvid_core::agent_memory::adapters::memvid_store::{InMemoryMemoryStore, MemoryStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::{MemoryLayer, MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::retrieval_candidates::{
    CandidatePool, CandidateScores, RetrievalCandidate,
};
use memvid_core::agent_memory::retrieval_planner::RetrievalPlanner;
use memvid_core::agent_memory::schemas::RetrievalQuery;

fn base_query(text: &str) -> RetrievalQuery {
    RetrievalQuery {
        query_text: text.to_string(),
        intent: QueryIntent::CurrentFact,
        entity: None,
        slot: None,
        scope: None,
        top_k: 10,
        as_of: None,
        include_expired: false,
        namespace_strict: false,
        user_id: None,
        project_id: None,
        task_id: None,
        thread_id: None,
    }
}

/// Compile-time check: all CandidatePool variants are reachable and pattern-matchable.
#[test]
fn candidate_pool_all_variants_accessible() {
    let pools = [
        CandidatePool::Lexical,
        CandidatePool::Vector,
        CandidatePool::Metadata,
        CandidatePool::Time,
        CandidatePool::Correction,
    ];
    for pool in &pools {
        let label = match pool {
            CandidatePool::Lexical => "lexical",
            CandidatePool::Vector => "vector",
            CandidatePool::Metadata => "metadata",
            CandidatePool::Time => "time",
            CandidatePool::Correction => "correction",
        };
        assert!(!label.is_empty());
    }
}

/// CandidatePool variants support equality and hashing (derived PartialEq + Eq + Hash).
#[test]
fn candidate_pool_supports_equality() {
    assert_eq!(CandidatePool::Lexical, CandidatePool::Lexical);
    assert_ne!(CandidatePool::Lexical, CandidatePool::Metadata);
    assert_ne!(CandidatePool::Time, CandidatePool::Correction);
    let mut set = std::collections::HashSet::new();
    set.insert(CandidatePool::Lexical);
    set.insert(CandidatePool::Lexical); // duplicate
    assert_eq!(
        set.len(),
        1,
        "HashSet must deduplicate identical CandidatePools"
    );
    set.insert(CandidatePool::Metadata);
    assert_eq!(set.len(), 2);
}

/// CandidateScores fields are all public and can be constructed and read.
#[test]
fn candidate_scores_fields_are_accessible() {
    let scores = CandidateScores {
        lexical_score: 0.75,
        vector_score: Some(0.9),
        entity_slot_match: true,
        recency: 0.5,
        salience: 0.8,
        confidence: 0.9,
        source_trust: 1.0,
        correction_status: Some("retracted".to_string()),
        scope_match: true,
    };
    assert!((scores.lexical_score - 0.75).abs() < f32::EPSILON);
    assert_eq!(scores.vector_score, Some(0.9));
    assert!(scores.entity_slot_match);
    assert_eq!(scores.correction_status.as_deref(), Some("retracted"));
    assert!(scores.scope_match);
}

/// CandidateScores with no vector score (the common case under default features).
#[test]
fn candidate_scores_without_vector_score() {
    let scores = CandidateScores {
        lexical_score: 0.5,
        vector_score: None,
        entity_slot_match: false,
        recency: 0.3,
        salience: 0.6,
        confidence: 0.7,
        source_trust: 0.8,
        correction_status: None,
        scope_match: false,
    };
    assert!(scores.vector_score.is_none());
    assert!(scores.correction_status.is_none());
    assert!(!scores.entity_slot_match);
    assert!(!scores.scope_match);
}

/// RetrievalCandidate produced by the planner has all expected public fields populated.
#[test]
fn planner_candidate_fields_are_populated() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
    store
        .put_memory(&durable(
            "agent",
            "preference",
            "verbose",
            "agent prefers verbose output",
            MemoryType::Preference, // → MemoryLayer::SelfModel
            SourceType::System,
            0.85,
            t,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    let query = base_query("verbose");
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    assert!(
        !result.candidates.is_empty(),
        "should surface at least one candidate"
    );
    let c: &RetrievalCandidate = &result.candidates[0];
    // memory_id must be a non-empty string
    assert!(!c.memory_id.is_empty(), "memory_id must be populated");
    // layer must be a valid MemoryLayer value
    let _layer: MemoryLayer = c.layer;
    // source_pools must be non-empty (lexical pool always contributes when text matches)
    assert!(
        !c.source_pools.is_empty(),
        "source_pools must be non-empty for a surfaced candidate"
    );
    // scores must have a finite lexical_score
    assert!(
        c.scores.lexical_score.is_finite(),
        "lexical_score must be finite"
    );
    // hit must carry the memory text
    assert!(!c.hit.text.is_empty(), "hit.text must not be empty");
}

/// Correction memory is attributed to CandidatePool::Correction, not Lexical or Metadata.
#[test]
fn correction_pool_sets_correction_pool_variant() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
    store
        .put_memory(&durable(
            "user",
            "job",
            "engineer",
            "correction: user is now an engineer",
            MemoryType::Correction, // → MemoryLayer::Correction
            SourceType::Chat,
            0.9,
            t,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    let mut query = base_query("zzz-no-lexical-match");
    query.entity = Some("user".to_string());
    query.slot = Some("job".to_string());
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    let correction_candidates: Vec<&RetrievalCandidate> = result
        .candidates
        .iter()
        .filter(|c| c.source_pools.contains(&CandidatePool::Correction))
        .collect();
    assert_eq!(
        correction_candidates.len(),
        1,
        "exactly one candidate should be from the correction pool"
    );
    // Correction layer should not appear in any Metadata-attributed candidate
    for c in &result.candidates {
        if c.source_pools.contains(&CandidatePool::Metadata) {
            assert_ne!(
                c.layer,
                MemoryLayer::Correction,
                "metadata pool must not scan the Correction layer"
            );
        }
    }
}

/// Entity/slot match on a durable memory sets entity_slot_match=true in CandidateScores.
#[test]
fn entity_slot_match_score_flag_set_for_metadata_hit() {
    let mut store = InMemoryMemoryStore::default();
    let t = ts(1_700_000_000);
    let clock = FixedClock::new(t);
    store
        .put_memory(&durable(
            "task",
            "priority",
            "high",
            "task priority set to high",
            MemoryType::GoalState, // → MemoryLayer::GoalState (in DURABLE_LAYERS)
            SourceType::Tool,
            0.8,
            t,
        ))
        .unwrap();
    let planner = RetrievalPlanner::new();
    let mut query = base_query("zzz-no-match");
    query.entity = Some("task".to_string());
    query.slot = Some("priority".to_string());
    let result = planner.plan(&mut store, &query, &clock).unwrap();
    let metadata_candidate = result
        .candidates
        .iter()
        .find(|c| c.source_pools.contains(&CandidatePool::Metadata))
        .expect("metadata pool candidate must exist");
    assert!(
        metadata_candidate.scores.entity_slot_match,
        "entity_slot_match must be true for a metadata pool hit derived via entity/slot filter"
    );
}
