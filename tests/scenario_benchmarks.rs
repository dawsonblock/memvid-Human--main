mod common;

use memvid_core::agent_memory::enums::{MemoryLayer, MemoryType, QueryIntent, Scope, SourceType};
use memvid_core::agent_memory::memory_compactor::CompactionMode;
use memvid_core::agent_memory::memory_feedback::FeedbackSignal;
use memvid_core::agent_memory::schemas::RetrievalQuery;

use common::{apply_durable, controller, durable, ts};

// ---------------------------------------------------------------------------
// Phase P — Concept Synthesis
// ---------------------------------------------------------------------------

/// Two belief-layer memories for the same entity should produce at least one
/// synthesised concept node.
#[test]
#[cfg(feature = "test_helpers")]
fn synthesizer_clusters_beliefs_into_concept_nodes() {
    let (mut ctrl, _sink) = controller(ts(1_700_000_000));

    let mut b1 = durable(
        "alice",
        "editor",
        "vim",
        "Alice uses vim",
        MemoryType::Fact,
        SourceType::Chat,
        0.9,
        ts(1_700_000_000),
    );
    b1.internal_layer = Some(MemoryLayer::Belief);

    let mut b2 = durable(
        "alice",
        "language",
        "rust",
        "Alice prefers Rust",
        MemoryType::Fact,
        SourceType::Chat,
        0.9,
        ts(1_700_000_100),
    );
    b2.internal_layer = Some(MemoryLayer::Belief);

    ctrl.put_memory_direct(&b1).unwrap();
    ctrl.put_memory_direct(&b2).unwrap();

    let result = ctrl.run_synthesis().unwrap();

    assert!(
        result.concepts_created >= 1,
        "expected at least one concept node; got concepts_created={}",
        result.concepts_created
    );
    assert!(
        !result.created_memory_ids.is_empty(),
        "expected created_memory_ids to be non-empty"
    );
}

/// Running synthesis twice on the same input must not create duplicate
/// concept nodes on the second pass (idempotency).
#[test]
#[cfg(feature = "test_helpers")]
fn synthesis_is_idempotent_across_two_passes() {
    let (mut ctrl, _sink) = controller(ts(1_700_000_000));

    let mut b1 = durable(
        "bob",
        "role",
        "engineer",
        "Bob is an engineer",
        MemoryType::Fact,
        SourceType::Chat,
        0.9,
        ts(1_700_000_000),
    );
    b1.internal_layer = Some(MemoryLayer::Belief);

    let mut b2 = durable(
        "bob",
        "team",
        "platform",
        "Bob works on the platform team",
        MemoryType::Fact,
        SourceType::Chat,
        0.9,
        ts(1_700_000_100),
    );
    b2.internal_layer = Some(MemoryLayer::Belief);

    ctrl.put_memory_direct(&b1).unwrap();
    ctrl.put_memory_direct(&b2).unwrap();

    let first = ctrl.run_synthesis().unwrap();
    assert!(
        first.concepts_created >= 1,
        "first pass should create concepts; got {}",
        first.concepts_created
    );

    let second = ctrl.run_synthesis().unwrap();
    assert_eq!(
        second.concepts_created, 0,
        "second pass with identical beliefs must not create duplicate concepts"
    );
}

/// At least two episode-layer memories tagged with `"procedure_step"` for
/// the same entity should be mined into a procedure memory.
#[test]
#[cfg(feature = "test_helpers")]
fn synthesizer_mines_procedure_from_tagged_episodes() {
    let (mut ctrl, _sink) = controller(ts(1_700_000_000));

    let mut s1 = durable(
        "deploy_bot",
        "step",
        "run tests",
        "step1: run tests",
        MemoryType::Episode,
        SourceType::System,
        0.8,
        ts(1_700_000_000),
    );
    s1.tags.push("procedure_step".to_string());

    let mut s2 = durable(
        "deploy_bot",
        "step",
        "build image",
        "step2: build image",
        MemoryType::Episode,
        SourceType::System,
        0.8,
        ts(1_700_000_100),
    );
    s2.tags.push("procedure_step".to_string());

    let mut s3 = durable(
        "deploy_bot",
        "step",
        "push to registry",
        "step3: push to registry",
        MemoryType::Episode,
        SourceType::System,
        0.8,
        ts(1_700_000_200),
    );
    s3.tags.push("procedure_step".to_string());

    ctrl.put_memory_direct(&s1).unwrap();
    ctrl.put_memory_direct(&s2).unwrap();
    ctrl.put_memory_direct(&s3).unwrap();

    let result = ctrl.run_synthesis().unwrap();

    assert!(
        result.procedures_mined >= 1,
        "expected at least one procedure mined; got procedures_mined={}",
        result.procedures_mined
    );
}

// ---------------------------------------------------------------------------
// Phase R — Feedback
// ---------------------------------------------------------------------------

/// Marking a memory as `Wrong` should set `metadata["feedback_wrong"] = "true"`.
#[test]
#[cfg(feature = "test_helpers")]
fn wrong_feedback_marks_memory_metadata() {
    let (mut ctrl, _sink) = controller(ts(1_700_000_000));

    let mem = durable(
        "user",
        "name",
        "alice",
        "The user's name is alice",
        MemoryType::Fact,
        SourceType::Chat,
        0.9,
        ts(1_700_000_000),
    );
    let id = apply_durable(&mut ctrl, &mem, None);

    ctrl.record_memory_feedback(&id, FeedbackSignal::Wrong, None)
        .unwrap();

    let stored = ctrl
        .get_memory_by_id(&id)
        .unwrap()
        .expect("memory should still exist after feedback");

    assert_eq!(
        stored.metadata.get("feedback_wrong").map(String::as_str),
        Some("true"),
        "Wrong feedback must set metadata[feedback_wrong] = true"
    );
}

/// Marking a memory as `Helpful` must NOT set the `feedback_wrong` flag.
#[test]
#[cfg(feature = "test_helpers")]
fn helpful_feedback_does_not_set_wrong_flag() {
    let (mut ctrl, _sink) = controller(ts(1_700_000_000));

    let mem = durable(
        "user",
        "pref",
        "dark-mode",
        "User prefers dark mode",
        MemoryType::Preference,
        SourceType::Chat,
        0.85,
        ts(1_700_000_000),
    );
    let id = apply_durable(&mut ctrl, &mem, None);

    ctrl.record_memory_feedback(&id, FeedbackSignal::Helpful, None)
        .unwrap();

    let stored = ctrl
        .get_memory_by_id(&id)
        .unwrap()
        .expect("memory should still exist after helpful feedback");

    assert!(
        !stored.metadata.contains_key("feedback_wrong"),
        "Helpful feedback must not set the feedback_wrong metadata key"
    );
}

/// A memory marked `Wrong` must not appear in subsequent `retrieve()` results.
#[test]
#[cfg(feature = "test_helpers")]
fn wrong_feedback_suppresses_memory_from_retrieve() {
    let (mut ctrl, _sink) = controller(ts(1_700_000_000));

    let mem = durable(
        "agent",
        "capability",
        "xyzzy-unique-token",
        "xyzzy-unique-token capability is available",
        MemoryType::Fact,
        SourceType::Chat,
        0.9,
        ts(1_700_000_000),
    );
    let id = apply_durable(&mut ctrl, &mem, None);

    ctrl.record_memory_feedback(&id, FeedbackSignal::Wrong, None)
        .unwrap();

    let hits = ctrl
        .retrieve(RetrievalQuery {
            query_text: "xyzzy-unique-token".into(),
            intent: QueryIntent::CurrentFact,
            entity: None,
            slot: None,
            scope: None,
            top_k: 20,
            as_of: None,
            include_expired: false,
            namespace_strict: false,
            user_id: None,
            project_id: None,
            task_id: None,
        })
        .unwrap();

    let found = hits.iter().any(|h| h.memory_id.as_deref() == Some(&id));
    assert!(
        !found,
        "memory marked Wrong should be excluded from retrieve results"
    );
}

// ---------------------------------------------------------------------------
// Phase Q — Namespace Scoping
// ---------------------------------------------------------------------------

/// A namespace-strict query for project "alpha" must exclude memories tagged
/// with `ns_project_id = "beta"`.
#[test]
#[cfg(feature = "test_helpers")]
fn namespace_strict_query_excludes_other_project_memories() {
    let (mut ctrl, _sink) = controller(ts(1_700_000_000));

    let mut mem_alpha = durable(
        "config",
        "indent",
        "2-spaces",
        "Use 2-space indentation for alpha",
        MemoryType::Instruction,
        SourceType::Chat,
        0.9,
        ts(1_700_000_000),
    );
    mem_alpha
        .metadata
        .insert("ns_project_id".into(), "alpha".into());
    let id_alpha = ctrl.put_memory_direct(&mem_alpha).unwrap();

    let mut mem_beta = durable(
        "config",
        "indent",
        "tabs",
        "Use tabs for beta",
        MemoryType::Instruction,
        SourceType::Chat,
        0.9,
        ts(1_700_000_100),
    );
    mem_beta
        .metadata
        .insert("ns_project_id".into(), "beta".into());
    let id_beta = ctrl.put_memory_direct(&mem_beta).unwrap();

    let hits = ctrl
        .retrieve(RetrievalQuery {
            query_text: "indentation".into(),
            intent: QueryIntent::CurrentFact,
            entity: None,
            slot: None,
            scope: None,
            top_k: 20,
            as_of: None,
            include_expired: false,
            namespace_strict: true,
            user_id: None,
            project_id: Some("alpha".into()),
            task_id: None,
        })
        .unwrap();

    let ids: Vec<_> = hits.iter().filter_map(|h| h.memory_id.as_deref()).collect();

    assert!(
        ids.contains(&id_alpha.as_str()),
        "alpha's memory must be included in namespace-strict alpha query"
    );
    assert!(
        !ids.contains(&id_beta.as_str()),
        "beta's memory must be excluded from namespace-strict alpha query"
    );
}

/// A `Scope::Shared` memory must survive a namespace-strict query for an
/// unrelated project because shared scope bypasses project filtering.
#[test]
#[cfg(feature = "test_helpers")]
fn shared_scope_memory_survives_namespace_strict_filter() {
    let (mut ctrl, _sink) = controller(ts(1_700_000_000));

    let mut shared_mem = durable(
        "platform",
        "disclaimer",
        "all-rights-reserved",
        "Global disclaimer: all rights reserved",
        MemoryType::Instruction,
        SourceType::System,
        0.9,
        ts(1_700_000_000),
    );
    shared_mem.scope = Scope::Shared;
    let id_shared = ctrl.put_memory_direct(&shared_mem).unwrap();

    let hits = ctrl
        .retrieve(RetrievalQuery {
            query_text: "disclaimer".into(),
            intent: QueryIntent::CurrentFact,
            entity: None,
            slot: None,
            scope: None,
            top_k: 20,
            as_of: None,
            include_expired: false,
            namespace_strict: true,
            user_id: None,
            project_id: Some("unrelated_project".into()),
            task_id: None,
        })
        .unwrap();

    let found = hits
        .iter()
        .any(|h| h.memory_id.as_deref() == Some(&id_shared));
    assert!(
        found,
        "Scope::Shared memory must be returned even in namespace-strict queries for other projects"
    );
}

// ---------------------------------------------------------------------------
// Phase O — Compaction
// ---------------------------------------------------------------------------

/// Running the compactor in Synthesize mode must complete without panicking,
/// returning a result with the correct mode tag.
#[test]
#[cfg(feature = "test_helpers")]
fn compact_synthesize_mode_runs_without_error() {
    let (mut ctrl, _sink) = controller(ts(1_700_000_000));

    // Seed belief-layer memories so the synthesizer has something to process.
    let mut b1 = durable(
        "carol",
        "timezone",
        "UTC",
        "Carol's timezone is UTC",
        MemoryType::Fact,
        SourceType::Chat,
        0.9,
        ts(1_700_000_000),
    );
    b1.internal_layer = Some(MemoryLayer::Belief);

    let mut b2 = durable(
        "carol",
        "locale",
        "en-GB",
        "Carol's locale is en-GB",
        MemoryType::Fact,
        SourceType::Chat,
        0.9,
        ts(1_700_000_100),
    );
    b2.internal_layer = Some(MemoryLayer::Belief);

    ctrl.put_memory_direct(&b1).unwrap();
    ctrl.put_memory_direct(&b2).unwrap();

    let result = ctrl.compact(CompactionMode::Synthesize).unwrap();

    assert_eq!(result.mode, CompactionMode::Synthesize);
    // summaries_created maps synthesis counts and must be non-negative (smoke test).
    assert!(
        result.summaries_created >= 0,
        "summaries_created should be a non-negative count"
    );
}

// ---------------------------------------------------------------------------
// Phase M — Salience ranking
// ---------------------------------------------------------------------------

/// When two memories match the same query, the one with higher salience
/// should appear at the top of the ranked results.
#[test]
#[cfg(feature = "test_helpers")]
fn higher_salience_memory_ranks_above_lower_salience() {
    let (mut ctrl, _sink) = controller(ts(1_700_000_000));

    let mut low_sal = durable(
        "robot",
        "greeting",
        "hello-low",
        "xylophone-unique-query robot greeting hello-low",
        MemoryType::Fact,
        SourceType::Chat,
        0.9,
        ts(1_700_000_000),
    );
    low_sal.salience = 0.1;

    let mut high_sal = durable(
        "robot",
        "greeting",
        "hello-high",
        "xylophone-unique-query robot greeting hello-high",
        MemoryType::Fact,
        SourceType::Chat,
        0.9,
        ts(1_700_000_100),
    );
    high_sal.salience = 0.95;

    let id_low = ctrl.put_memory_direct(&low_sal).unwrap();
    let id_high = ctrl.put_memory_direct(&high_sal).unwrap();

    let hits = ctrl
        .retrieve(RetrievalQuery {
            query_text: "xylophone-unique-query".into(),
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
        })
        .unwrap();

    assert!(
        hits.len() >= 2,
        "both memories should be returned; got {} hits",
        hits.len()
    );

    let pos_high = hits
        .iter()
        .position(|h| h.memory_id.as_deref() == Some(&id_high));
    let pos_low = hits
        .iter()
        .position(|h| h.memory_id.as_deref() == Some(&id_low));

    if let (Some(ph), Some(pl)) = (pos_high, pos_low) {
        assert!(
            ph < pl,
            "high-salience memory (pos {ph}) should rank above low-salience memory (pos {pl})"
        );
    } else {
        // At minimum both hits must have been located
        assert!(
            pos_high.is_some(),
            "high-salience memory id={id_high} not found in retrieve results"
        );
        assert!(
            pos_low.is_some(),
            "low-salience memory id={id_low} not found in retrieve results"
        );
    }
}
