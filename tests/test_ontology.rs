//! Integration tests for the ontology registry (Phase 2 — Ontology Stabilisation).

use memvid_core::agent_memory::ontology::{
    ConceptCanonicalizer, ConceptMergeDecision, OntologyRegistry,
};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn now_ts() -> i64 {
    1_700_000_000_i64
}

fn reg_with(id: &str, text: &str, entity: &str, confidence: f32) -> OntologyRegistry {
    let mut r = OntologyRegistry::new();
    r.register(id, text, entity, confidence, now_ts());
    r
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// A concept registered under one ID must be retrievable by alias lookup on
/// its canonical text (or a normalised variant of it).
#[test]
fn alias_resolution_returns_canonical_id() {
    let mut registry = OntologyRegistry::new();
    registry.register("c1", "Rust Language", "tech", 0.9, now_ts());

    // resolve_alias normalises input so casing / whitespace variations match.
    let resolved = registry.resolve_alias("rust language");
    assert_eq!(resolved, Some("c1"), "Expected canonical ID 'c1'");
}

/// Registering the same (normalised) concept text under a fresh ID must return
/// the *existing* canonical ID — the registry must not add a duplicate entry.
#[test]
fn duplicate_concept_merges_into_existing_canonical() {
    let mut registry = OntologyRegistry::new();
    let first = registry.register("c1", "python web framework", "tech", 0.8, now_ts());

    // Same text, different requested ID — should return existing canonical.
    let second = registry.register("c2", "Python Web Framework", "tech", 0.85, now_ts());

    assert_eq!(
        first, second,
        "Duplicate text must resolve to the same canonical ID"
    );
    assert_eq!(registry.len(), 1, "Registry must contain exactly one entry");
}

/// After a confidence drop exceeding the drift threshold the version history
/// must record the previous value.
#[test]
fn concept_drift_detected_after_confidence_drop() {
    let mut registry = OntologyRegistry::new();
    registry.register("c1", "machine learning pipeline", "infra", 0.9, now_ts());

    // Drop confidence by more than default threshold (0.1).
    registry.add_supporting_memory("c1", "m1", Some(0.5), "retraction", now_ts() + 1);

    let entry = registry.get("c1").expect("entry must exist");
    assert!(
        !entry.version_history.entries().is_empty(),
        "Version history must record the drift event"
    );
    assert!(
        (entry.confidence - 0.5).abs() < f32::EPSILON,
        "Confidence must be updated to new value"
    );
}

/// Attempting to merge a concept with itself must be rejected.
#[test]
fn cyclic_concept_merge_rejected() {
    let mut registry = reg_with("c1", "data pipeline", "infra", 0.8);
    let result = registry.propose_merge(
        "c1",
        "c1",
        ConceptMergeDecision::Merge,
        "self",
        "m1",
        now_ts(),
    );
    assert!(result.is_err(), "Self-merge must return Err");
}

/// Two genuinely distinct concepts kept with `PreserveSeparate` must both
/// remain as independent entries — neither must be superseded.
#[test]
fn conflicting_abstraction_preserved_separate() {
    let mut registry = OntologyRegistry::new();
    registry.register("c1", "supervised learning", "ml", 0.85, now_ts());
    registry.register("c2", "unsupervised learning", "ml", 0.80, now_ts());

    registry
        .propose_merge(
            "c1",
            "c2",
            ConceptMergeDecision::PreserveSeparate,
            "distinct subfields",
            "merge1",
            now_ts(),
        )
        .unwrap();

    assert!(
        registry.get("c1").unwrap().superseded_by.is_none(),
        "c1 must not be retired"
    );
    assert!(
        registry.get("c2").unwrap().superseded_by.is_none(),
        "c2 must not be retired"
    );
    assert_eq!(registry.len(), 2, "Both entries must still exist");
}

/// After a `Supersede` merge the superseded entry must record `superseded_by`
/// and still be retrievable (provenance is retained).
#[test]
fn superseded_concept_retains_provenance() {
    let mut registry = OntologyRegistry::new();
    registry.register("old", "legacy service mesh", "infra", 0.7, now_ts());
    registry.register("new", "modern service mesh", "infra", 0.9, now_ts());

    registry
        .propose_merge(
            "old",
            "new",
            ConceptMergeDecision::Supersede,
            "old approach subsumed",
            "m1",
            now_ts(),
        )
        .unwrap();

    let retired = registry
        .get("old")
        .expect("retired entry must still be accessible");
    assert_eq!(
        retired.superseded_by.as_deref(),
        Some("new"),
        "superseded_by must point to new canonical"
    );
    assert!(retired.is_retired(), "is_retired() must return true");
}

/// Version history must stop accumulating entries beyond 50.
#[test]
fn version_history_bounded_at_fifty_entries() {
    let mut registry = OntologyRegistry::new();
    registry.register("c1", "evolving concept", "test", 1.0, now_ts());

    // Alternate confidence between [0.1 .. 0.9] to force 60 drift events.
    for i in 0..60_u32 {
        let conf = if i % 2 == 0 { 0.9_f32 } else { 0.1_f32 };
        registry.add_supporting_memory(
            "c1",
            format!("m{i}"),
            Some(conf),
            "update",
            now_ts() + i as i64,
        );
    }

    let entry = registry.get("c1").expect("entry must exist");
    assert!(
        entry.version_history.entries().len() <= 50,
        "Version history must not exceed 50 entries (got {})",
        entry.version_history.entries().len()
    );
}

/// `find_by_entity` must return all concepts registered for a given entity.
#[test]
fn registry_find_by_entity_returns_all_entries() {
    let mut registry = OntologyRegistry::new();
    registry.register("c1", "deploy pipeline", "infra", 0.8, now_ts());
    registry.register("c2", "monitoring stack", "infra", 0.75, now_ts());
    registry.register("c3", "frontend framework", "frontend", 0.9, now_ts());

    let infra = registry.find_by_entity("infra");
    assert_eq!(
        infra.len(),
        2,
        "Expected 2 infra entries, got {}",
        infra.len()
    );
    assert!(infra.iter().any(|e| e.canonical_id == "c1"));
    assert!(infra.iter().any(|e| e.canonical_id == "c2"));
}

/// `ConceptCanonicalizer::normalize` must produce identical output for
/// equivalent texts that differ only in order, casing, and duplicate tokens.
#[test]
fn canonicalizer_normalises_equivalent_texts() {
    let c = ConceptCanonicalizer;
    let a = c.normalize("Rust Language Framework");
    let b = c.normalize("framework rust language");
    assert_eq!(
        a, b,
        "Normalised texts must be equal regardless of token order"
    );
}
