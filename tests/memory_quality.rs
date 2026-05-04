//! Phase H — Memory Quality Integration Tests
//! Covers: false_memory_rejection, correction_persistence, context_scoping,
//! semantic_contradiction, preference_change, profile_compaction,
//! retrieval_precision, stale_memory_suppression, project_boundary,
//! reasoning_cycle, procedure_recall.

mod common;

use memvid_core::agent_memory::adapters::memvid_store::MemoryStore;
use memvid_core::agent_memory::belief_conflict_resolver::{
    BeliefConflictResolution, BeliefConflictResolver, ConflictContext,
};
use memvid_core::agent_memory::enums::{MemoryLayer, MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::memory_compactor::CompactionMode;
use memvid_core::agent_memory::schemas::{IngestContext, RetrievalQuery};

use common::{candidate, controller, durable, ts};

// ── 1. false_memory_rejection ─────────────────────────────────────────────────

#[test]
#[cfg(feature = "test_helpers")]
fn false_memory_rejection_low_confidence() {
    let (mut ctrl, _) = controller(ts(1_700_000_000));
    let mut c = candidate("user", "mood", "happy", "User seems happy today.");
    c.confidence = 0.05;
    assert_eq!(
        ctrl.ingest(c).unwrap(),
        None,
        "low-confidence candidate must be rejected"
    );
}

#[test]
#[cfg(feature = "test_helpers")]
fn false_memory_rejection_low_salience() {
    let (mut ctrl, _) = controller(ts(1_700_000_000));
    let mut c = candidate("user", "mood", "happy", "User seems happy today.");
    c.salience = 0.10;
    assert_eq!(
        ctrl.ingest(c).unwrap(),
        None,
        "low-salience candidate must be rejected"
    );
}

#[test]
#[cfg(feature = "test_helpers")]
fn false_memory_rejection_empty_text() {
    let (mut ctrl, _) = controller(ts(1_700_000_000));
    let mut c = candidate("user", "mood", "happy", "");
    assert_eq!(
        ctrl.ingest(c).unwrap(),
        None,
        "empty raw_text must be rejected"
    );
}

// ── 2. correction_persistence ─────────────────────────────────────────────────

#[test]
#[cfg(feature = "test_helpers")]
fn correction_persists_after_ingest() {
    let (mut ctrl, _) = controller(ts(1_700_000_000));
    // ≥2 CORRECTION_HINTS trigger classifier → MemoryType::Correction
    // "actually" + "not" each appear in the hint list.
    let mut c = candidate(
        "user",
        "name",
        "Alice",
        "Actually, that is not right. The user's name is Alice.",
    );
    c.memory_type = MemoryType::Correction;
    // Admission fast-accepts Correction types regardless of confidence/salience.
    let id = ctrl
        .ingest(c)
        .unwrap()
        .expect("correction candidate must be stored");
    let mems = ctrl.store().memories();
    assert!(
        mems.iter()
            .any(|m| m.memory_id == id && m.memory_type == MemoryType::Correction),
        "stored memory must have MemoryType::Correction"
    );
}

// ── 3. context_scoping ────────────────────────────────────────────────────────

#[test]
#[cfg(feature = "test_helpers")]
fn ingest_text_stores_project_id_in_metadata() {
    let (mut ctrl, _) = controller(ts(1_700_000_000));
    let ctx = IngestContext {
        project_id: Some("proj-42".to_string()),
        ..IngestContext::default()
    };
    // "always" triggers ClaimExtractor instruction hint → Instruction type → SelfModel layer → promoted to durable memory.
    ctrl.ingest_text(
        "Remember this: always use dark mode for the user interface.",
        ctx,
    )
    .unwrap();
    let mems = ctrl.store().memories();
    let has_project = mems
        .iter()
        .any(|m| m.metadata.get("ns_project_id").map(String::as_str) == Some("proj-42"));
    assert!(
        has_project,
        "ns_project_id=proj-42 must be stored in memory metadata"
    );
}

// ── 4. semantic_contradiction ─────────────────────────────────────────────────

#[test]
fn semantic_contradiction_not_prefix_resolves_to_contradicts() {
    // "not apple" triggers the `not {token}` rule — token "apple" appears in existing.
    let ctx = ConflictContext {
        existing: "apple silicon".to_string(),
        incoming: "not apple silicon".to_string(),
        source_type: SourceType::Chat,
    };
    assert_eq!(
        BeliefConflictResolver::resolve("apple silicon", "not apple silicon", &ctx),
        BeliefConflictResolution::Contradicts
    );
}

#[test]
fn semantic_contradiction_that_is_wrong_resolves_to_contradicts() {
    let ctx = ConflictContext {
        existing: "RTX 3080".to_string(),
        incoming: "that is wrong, i use apple silicon".to_string(),
        source_type: SourceType::Chat,
    };
    assert_eq!(
        BeliefConflictResolver::resolve("RTX 3080", "that is wrong, i use apple silicon", &ctx),
        BeliefConflictResolution::Contradicts
    );
}

#[test]
fn unrelated_hardware_values_resolve_to_ambiguous() {
    // Two distinct GPU/CPU values with no shared tokens → Ambiguous (not Contradicts).
    let ctx = ConflictContext {
        existing: "RTX 3080".to_string(),
        incoming: "Apple Silicon".to_string(),
        source_type: SourceType::Chat,
    };
    assert_eq!(
        BeliefConflictResolver::resolve("RTX 3080", "Apple Silicon", &ctx),
        BeliefConflictResolution::Ambiguous,
        "values with no shared tokens and no contradiction markers are Ambiguous not Contradicts"
    );
}

// ── 5. preference_change ──────────────────────────────────────────────────────

#[test]
fn contextual_preference_classified_as_compatible_variant() {
    // "for this project" is a scope qualifier → CompatibleContextualVariant.
    let ctx = ConflictContext {
        existing: "dark mode".to_string(),
        incoming: "for this project i prefer light mode".to_string(),
        source_type: SourceType::Chat,
    };
    assert_eq!(
        BeliefConflictResolver::resolve("dark mode", "for this project i prefer light mode", &ctx),
        BeliefConflictResolution::CompatibleContextualVariant
    );
}

#[test]
fn globally_superseded_preference_resolves_to_supersedes() {
    // "from now on" is a supersedes marker.
    let ctx = ConflictContext {
        existing: "dark mode".to_string(),
        incoming: "from now on always use light mode".to_string(),
        source_type: SourceType::Chat,
    };
    assert_eq!(
        BeliefConflictResolver::resolve("dark mode", "from now on always use light mode", &ctx),
        BeliefConflictResolution::Supersedes
    );
}

// ── 6. profile_compaction ─────────────────────────────────────────────────────

#[test]
#[cfg(feature = "test_helpers")]
fn profile_compaction_deduplicates_identical_slot_values() {
    let (mut ctrl, _) = controller(ts(1_700_000_000));
    // Insert 5 memories with identical entity+slot+value, varying confidence.
    for i in 0..5u32 {
        let mut mem = durable(
            "user",
            "device",
            "laptop",
            "User uses a laptop.",
            MemoryType::Episode,
            SourceType::Chat,
            0.5 + (i as f32 * 0.01),
            ts(1_699_990_000),
        );
        mem.memory_id = format!("dup-{i}");
        mem.internal_layer = Some(MemoryLayer::Episode);
        ctrl.put_memory_direct(&mem).unwrap();
    }
    assert_eq!(
        ctrl.store().memories().len(),
        5,
        "all 5 inserted before compaction"
    );
    let result = ctrl.compact(CompactionMode::Dedupe).unwrap();
    assert!(
        result.deduplicated_count >= 4,
        "at least 4 duplicates removed; got {}",
        result.deduplicated_count
    );
}

// ── 7. retrieval_precision ────────────────────────────────────────────────────

#[test]
#[cfg(feature = "test_helpers")]
fn retrieve_context_places_correction_in_corrections_bucket() {
    let (mut ctrl, _) = controller(ts(1_700_000_000));
    // Use ≥2 correction hints so the classifier keeps MemoryType::Correction.
    // "actually" + "not" → correction_signal ≥ 2 → Correction layer.
    let mut c = candidate(
        "user",
        "name",
        "Bob",
        "Actually, that is not right — the user's name is Bob.",
    );
    c.memory_type = MemoryType::Correction;
    ctrl.ingest(c).unwrap();

    let query = RetrievalQuery {
        query_text: "user name".to_string(),
        intent: QueryIntent::EpisodicRecall,
        entity: Some("user".to_string()),
        slot: Some("name".to_string()),
        scope: None,
        top_k: 10,
        as_of: None,
        include_expired: false,
        namespace_strict: false,
        user_id: None,
        project_id: None,
        task_id: None,
        thread_id: None,
    };
    let packet = ctrl.retrieve_context(query).unwrap();
    assert!(
        !packet.corrections.is_empty(),
        "correction-type memory must appear in corrections bucket"
    );
}

// ── 8. stale_memory_suppression ───────────────────────────────────────────────

#[test]
#[cfg(feature = "test_helpers")]
fn expired_memory_routed_to_stale_items_bucket() {
    const NOW: i64 = 1_700_000_000;
    let (mut ctrl, _) = controller(ts(NOW));

    // stored_at is 10000 seconds in the past, TTL = 1 second → expired.
    let mut mem = durable(
        "user",
        "location",
        "Berlin",
        "User is currently in Berlin.",
        MemoryType::Fact,
        SourceType::Chat,
        0.9,
        ts(NOW - 10_000),
    );
    mem.memory_id = "stale-mem-1".to_string();
    mem.ttl = Some(1); // expires 1 second after stored_at
    mem.internal_layer = Some(MemoryLayer::Episode); // Episode layer so episodic_hits() finds it and evaluates TTL-based expiry
    ctrl.put_memory_direct(&mem).unwrap();
    // Explicitly mark the memory as expired so InMemoryMemoryStore::search()
    // also returns expired=true (the search path uses the expired HashSet,
    // not TTL; both paths must agree for the hit to survive dedup as expired).
    ctrl.store_mut().expire_memory("stale-mem-1").unwrap();

    let query = RetrievalQuery {
        query_text: "location".to_string(),
        intent: QueryIntent::EpisodicRecall,
        entity: None,
        slot: None,
        scope: None,
        top_k: 10,
        as_of: None,
        include_expired: true,
        namespace_strict: false,
        user_id: None,
        project_id: None,
        task_id: None,
        thread_id: None,
    };
    let packet = ctrl.retrieve_context(query).unwrap();
    assert!(
        packet
            .stale_items
            .iter()
            .any(|h| h.memory_id.as_deref() == Some("stale-mem-1")),
        "expired memory must be routed to stale_items bucket"
    );
}

// ── 9. project_boundary ───────────────────────────────────────────────────────

#[test]
#[cfg(feature = "test_helpers")]
fn project_a_instruction_carries_project_a_namespace() {
    let (mut ctrl, _) = controller(ts(1_700_000_000));
    let ctx_a = IngestContext {
        project_id: Some("project-A".to_string()),
        ..IngestContext::default()
    };
    ctrl.ingest_text(
        "Remember this: always use tabs for indentation in this project.",
        ctx_a,
    )
    .unwrap();

    let mems = ctrl.store().memories();
    let proj_a_mems: Vec<_> = mems
        .iter()
        .filter(|m| m.metadata.get("ns_project_id").map(String::as_str) == Some("project-A"))
        .collect();
    assert!(
        !proj_a_mems.is_empty(),
        "project-A instruction must be stored with ns_project_id=project-A"
    );
}

#[test]
#[cfg(feature = "test_helpers")]
fn two_projects_store_separate_namespace_tags() {
    let (mut ctrl, _) = controller(ts(1_700_000_000));

    let ctx_a = IngestContext {
        project_id: Some("project-A".to_string()),
        ..IngestContext::default()
    };
    ctrl.ingest_text("Remember this: always use tabs in project A.", ctx_a)
        .unwrap();

    let ctx_b = IngestContext {
        project_id: Some("project-B".to_string()),
        ..IngestContext::default()
    };
    ctrl.ingest_text("Remember this: always use spaces in project B.", ctx_b)
        .unwrap();

    let mems = ctrl.store().memories();
    let count_a = mems
        .iter()
        .filter(|m| m.metadata.get("ns_project_id").map(String::as_str) == Some("project-A"))
        .count();
    let count_b = mems
        .iter()
        .filter(|m| m.metadata.get("ns_project_id").map(String::as_str) == Some("project-B"))
        .count();
    assert!(count_a >= 1, "at least one memory for project-A");
    assert!(count_b >= 1, "at least one memory for project-B");
}

// ── 10. reasoning_cycle ───────────────────────────────────────────────────────

#[test]
#[cfg(feature = "test_helpers")]
fn reasoning_cycle_emits_reflection_for_repeated_slot() {
    const NOW: i64 = 1_700_000_000;
    const RECENT: i64 = NOW - 3600; // 1 hour ago — within the 24 h window.
    let (mut ctrl, _) = controller(ts(NOW));

    // Insert 3 Episode memories for the same (entity, slot) within the 24 h window.
    for i in 0..3u32 {
        let mut mem = durable(
            "user",
            "mood",
            &format!("state-{i}"),
            "User mood was noted.",
            MemoryType::Episode,
            SourceType::Chat,
            0.8,
            ts(RECENT),
        );
        mem.memory_id = format!("ep-mood-{i}");
        mem.internal_layer = Some(MemoryLayer::Episode);
        ctrl.put_memory_direct(&mem).unwrap();
    }

    let result = ctrl.run_reasoning_cycle().unwrap();
    assert!(
        !result.reflections.is_empty(),
        "reasoning cycle must emit at least one reflection for user.mood (3 episodes)"
    );
    assert!(
        result
            .reflections
            .iter()
            .any(|r| r.contains("user") && r.contains("mood")),
        "reflection must mention 'user' and 'mood'"
    );
}

// ── 11. procedure_recall ──────────────────────────────────────────────────────

#[test]
#[cfg(feature = "test_helpers")]
fn procedure_recall_runs_without_panic() {
    let (mut ctrl, _) = controller(ts(1_700_000_000));
    // Ingest several candidates describing procedural steps.
    for i in 0..3u32 {
        let c = candidate(
            "workflow",
            "step",
            "deploy",
            &format!("Step {i}: run the deploy script and verify output."),
        );
        ctrl.ingest(c).ok();
    }
    // Primary assertion: no panic; secondary: API returns a Vec.
    let procedures = ctrl.list_procedures().unwrap();
    let _ = procedures.len(); // ensure it's usable
}
