mod common;

use std::sync::Arc;

use memvid_core::agent_memory::adapters::memvid_store::{InMemoryMemoryStore, MemoryStore};
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::{MemoryLayer, MemoryType};
use memvid_core::agent_memory::reflection_governance::{
    FilteredCycleResult, ReflectionCandidate, ReflectionDecay, ReflectionEvidenceThreshold,
    ReflectionSafetyPolicy, ReflectionValidationLayer,
};

use common::ts;

// ── helpers ──────────────────────────────────────────────────────────────────

fn candidate_with(
    text: &str,
    ids: Vec<&str>,
    confidences: Vec<f32>,
    origin_rule: &str,
) -> ReflectionCandidate {
    ReflectionCandidate {
        text: text.to_string(),
        supporting_memory_ids: ids.into_iter().map(str::to_string).collect(),
        supporting_confidences: confidences,
        origin_rule: origin_rule.to_string(),
    }
}

fn default_layer() -> ReflectionValidationLayer {
    ReflectionValidationLayer::new(ReflectionSafetyPolicy::default())
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// A candidate backed by only one memory (< minimum 2) must be rejected.
#[test]
fn runaway_reflection_prevented_below_evidence_minimum() {
    let layer = default_layer();
    let candidates = vec![candidate_with(
        "Agent prefers short answers",
        vec!["m-001"],
        vec![0.9],
        "slot-frequency",
    )];
    let result = layer.validate(&candidates);
    assert!(
        result.passed.is_empty(),
        "single-evidence candidate should not pass"
    );
    assert_eq!(result.rejected.len(), 1);
    assert!(
        result.rejected[0].1.contains("insufficient evidence"),
        "rejection reason should mention evidence: {}",
        result.rejected[0].1
    );
}

/// A candidate with zero supporting memories must be rejected.
#[test]
fn unsupported_profile_synthesis_rejected() {
    let layer = default_layer();
    let candidates = vec![candidate_with(
        "User dislikes verbose explanations",
        vec![],
        vec![],
        "slot-frequency",
    )];
    let result = layer.validate(&candidates);
    assert!(result.passed.is_empty());
    assert_eq!(result.rejected.len(), 1);
}

/// When max_reflections_per_cycle is 1, only the first valid candidate passes.
#[test]
fn recursive_concept_amplification_blocked() {
    let policy = ReflectionSafetyPolicy {
        max_reflections_per_cycle: 1,
        ..ReflectionSafetyPolicy::default()
    };
    let layer = ReflectionValidationLayer::new(policy);

    // Build 5 valid candidates (5 supporting memories each at 0.9 confidence →
    // coverage_factor = 5/5 = 1.0, score = 0.9 ≥ threshold 0.6).
    let candidates: Vec<ReflectionCandidate> = (0..5)
        .map(|i| {
            candidate_with(
                &format!("Pattern {i}"),
                vec!["m1", "m2", "m3", "m4", "m5"],
                vec![0.9, 0.9, 0.9, 0.9, 0.9],
                "slot-frequency",
            )
        })
        .collect();

    let result = layer.validate(&candidates);
    assert_eq!(
        result.passed.len(),
        1,
        "only one candidate should pass the cycle cap"
    );
}

/// A candidate whose mean confidence (0.1) is below the evidence threshold (0.6) is rejected.
#[test]
fn false_abstraction_suppressed_below_confidence_threshold() {
    let layer = default_layer();
    let candidates = vec![candidate_with(
        "Agent frequently mentions weather",
        vec!["m1", "m2", "m3"],
        vec![0.1, 0.1, 0.1],
        "slot-frequency",
    )];
    let result = layer.validate(&candidates);
    assert!(
        result.passed.is_empty(),
        "low-confidence candidate should be rejected"
    );
    assert_eq!(result.rejected.len(), 1);
    assert!(
        result.rejected[0].1.contains("confidence"),
        "rejection reason should mention confidence: {}",
        result.rejected[0].1
    );
}

/// A candidate supported by fewer than 5 memories receives the short TTL (30 days).
#[test]
fn stale_reflection_expires_after_short_ttl() {
    let candidate = candidate_with(
        "Pattern detected: agent.preference appeared 3 times in the last 24 h",
        vec!["m1", "m2", "m3"],
        vec![0.8, 0.8, 0.8],
        "slot-frequency",
    );
    let ttl = ReflectionDecay::ttl_for(&candidate);
    assert_eq!(
        ttl,
        ReflectionDecay::TTL_UNSUPPORTED_SECS,
        "3 supporting memories → 30-day TTL"
    );
}

/// A valid reflection candidate is persisted as a Trace memory with reversibility metadata.
#[test]
fn valid_reflection_persisted_as_trace_with_metadata() {
    let mut store = InMemoryMemoryStore::default();
    let clock = FixedClock::new(ts(1_700_000_000));
    let layer = default_layer();

    let candidates = vec![candidate_with(
        "Pattern detected: agent.preference appeared 4 times in the last 24 h",
        vec!["m1", "m2", "m3", "m4"],
        vec![0.85, 0.85, 0.85, 0.85],
        "slot-frequency",
    )];
    let filtered = layer.validate(&candidates);
    assert_eq!(filtered.passed.len(), 1, "valid candidate should pass");

    let ids = layer
        .write_reflections_to_store(&filtered, &mut store, &clock)
        .expect("write succeeds");
    assert_eq!(ids.len(), 1);

    let written = store
        .get_memory(&ids[0])
        .expect("store read succeeds")
        .expect("memory should exist");

    assert_eq!(written.memory_type, MemoryType::Trace, "persisted as Trace");
    assert_eq!(
        written.internal_layer,
        Some(MemoryLayer::Trace),
        "internal_layer is Trace"
    );
    assert!(
        written.tags.contains(&"reflection".to_string()),
        "should be tagged 'reflection'"
    );
    assert_eq!(
        written
            .metadata
            .get("reflection_reversible")
            .map(String::as_str),
        Some("true"),
        "should be marked reversible"
    );
    assert_eq!(
        written
            .metadata
            .get("reflection_origin")
            .map(String::as_str),
        Some("slot-frequency"),
        "should record origin rule"
    );
    assert!(
        written.metadata.contains_key("reflection_at"),
        "should record reflection timestamp"
    );
    assert_eq!(
        written.ttl,
        Some(ReflectionDecay::TTL_UNSUPPORTED_SECS),
        "4 supporting memories → 30-day TTL"
    );
}
