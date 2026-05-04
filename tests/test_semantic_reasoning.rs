//! Semantic reasoning integration tests for [`SemanticArbitrationEngine`].
//!
//! All six tests exercise the engine directly, covering rule-based short-
//! circuits, embedding-pass resolution, and multi-step belief convergence.

mod common;

use memvid_core::agent_memory::belief_conflict_resolver::BeliefConflictResolution;
use memvid_core::agent_memory::embedding_provider::AgentEmbeddingProvider;
use memvid_core::agent_memory::errors::Result;
use memvid_core::agent_memory::semantic_arbitration::{
    ArbitrationInput, SemanticArbitrationEngine,
};

// ── Mock embedding providers ──────────────────────────────────────────────────

/// Returns pre-scripted vectors keyed by the exact input text.
/// Used for test 3 where we need a known cosine in the Reinforces band.
#[derive(Debug)]
struct ScriptedEmbedder {
    pairs: Vec<(String, Vec<f32>)>,
    default_vec: Vec<f32>,
}

impl ScriptedEmbedder {
    fn new(pairs: Vec<(&str, Vec<f32>)>, default_vec: Vec<f32>) -> Self {
        ScriptedEmbedder {
            pairs: pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            default_vec,
        }
    }
}

impl AgentEmbeddingProvider for ScriptedEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        for (key, vec) in &self.pairs {
            if key == text {
                return Ok(vec.clone());
            }
        }
        Ok(self.default_vec.clone())
    }
    fn dim(&self) -> usize {
        self.default_vec.len()
    }
}

/// Panics immediately if `embed()` is called.
/// Used to prove that a rule-based short-circuit never invokes the embedder.
#[derive(Debug)]
struct PanickingEmbedder;

impl AgentEmbeddingProvider for PanickingEmbedder {
    fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        panic!("embedder must not be called when rules resolve without ambiguity");
    }
    fn dim(&self) -> usize {
        2
    }
}

// ── Test helpers ──────────────────────────────────────────────────────────────

/// Unit-normalised vector along the given 2-D angle (in radians).
fn unit_2d(radians: f64) -> Vec<f32> {
    vec![radians.cos() as f32, radians.sin() as f32]
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// High Jaccard overlap (≥ 0.8) is resolved as `Reinforces` by the rules pass.
///
/// "user prefers dark mode" ∩ "user prefers dark mode interface" shares 4 of
/// 5 tokens → Jaccard = 0.80, which is exactly the minimum threshold.
#[test]
fn implicit_preference_inferred_from_repeated_episode_pattern() {
    let input = ArbitrationInput::new(
        "user prefers dark mode",
        "user prefers dark mode interface",
        "user",
        "ui_preference",
    );
    let outcome = SemanticArbitrationEngine::arbitrate(&input, None, None);
    assert_eq!(
        outcome.resolution,
        BeliefConflictResolution::Reinforces,
        "expected Reinforces from Jaccard ≥ 0.80"
    );
    assert_eq!(outcome.resolver_name, "rules");
}

/// "actually " prefix is recognised as a superseding correction.
#[test]
fn contextual_correction_understood_as_update() {
    let input = ArbitrationInput::new(
        "use Rust for all backend services",
        "actually Python is preferred for this codebase",
        "project",
        "backend_language",
    );
    let outcome = SemanticArbitrationEngine::arbitrate(&input, None, None);
    assert_eq!(
        outcome.resolution,
        BeliefConflictResolution::Supersedes,
        "expected Supersedes for 'actually' prefix"
    );
    assert_eq!(outcome.resolver_name, "rules");
}

/// A fully rules-ambiguous pair falls through to the embedding pass.
///
/// Vectors are chosen so their cosine ≈ 0.85, which sits inside the
/// Reinforces band (COSINE_REINFORCE=0.78 … COSINE_SAME=0.92].
#[test]
fn ambiguous_instruction_falls_through_to_embedding_pass() {
    // Unit vectors: angle 0 rad and angle ≈ 0.5548 rad give cos ≈ 0.85.
    //   cos(0.5548) ≈ 0.8500, sin(0.5548) ≈ 0.5268  (norm = 1.0)
    let vec_a = unit_2d(0.0); // [1.0, 0.0]
    let vec_b = unit_2d(0.5548_f64); // [~0.85, ~0.527] — cosine with vec_a ≈ 0.85

    let a_text = "I enjoy structured workflows";
    let b_text = "productive organisation of work";

    let embedder = ScriptedEmbedder::new(
        vec![(a_text, vec_a), (b_text, vec_b)],
        vec![0.0_f32, 1.0_f32],
    );

    let input = ArbitrationInput::new(a_text, b_text, "agent", "work_style");
    let outcome = SemanticArbitrationEngine::arbitrate(&input, Some(&embedder), None);

    assert_eq!(
        outcome.resolution,
        BeliefConflictResolution::Reinforces,
        "cosine ≈ 0.85 should map to Reinforces"
    );
    assert_eq!(outcome.resolver_name, "embedding");
    let sim = outcome.cosine_similarity.expect("cosine must be recorded");
    assert!(
        sim > 0.78 && sim <= 0.92,
        "cosine {sim:.4} should be in Reinforces band (0.78, 0.92]"
    );
}

/// When the rules pass resolves without ambiguity the embedding provider is
/// never invoked.  The `PanickingEmbedder` proves this statically.
#[test]
fn non_ambiguous_rule_short_circuits_embedding() {
    let input = ArbitrationInput::new("dark mode", "dark mode", "user", "theme");
    // Identical strings → rules returns Same immediately; embedder never called.
    let outcome = SemanticArbitrationEngine::arbitrate(&input, Some(&PanickingEmbedder), None);
    assert_eq!(outcome.resolution, BeliefConflictResolution::Same);
    assert_eq!(outcome.resolver_name, "rules");
}

/// "not {token}" pattern fires `Contradicts` when the token appears in the
/// existing belief value.
///
/// The negative outcome text "not python" contains the token "python" which
/// is present in the existing value — this is the latent constraint that
/// the engine extracts.
#[test]
fn latent_constraint_extracted_from_negative_outcome() {
    let input = ArbitrationInput::new(
        "use python for all scripts",
        "not python for that task",
        "project",
        "scripting_language",
    );
    let outcome = SemanticArbitrationEngine::arbitrate(&input, None, None);
    assert_eq!(
        outcome.resolution,
        BeliefConflictResolution::Contradicts,
        "expected Contradicts for 'not python' pattern"
    );
    assert_eq!(outcome.resolver_name, "rules");
}

/// A three-step belief chain converges to a canonical final value.
///
/// 1. "Rust" ← "actually Python now"   → Supersedes  (incoming wins)
/// 2. new incumbent "actually Python now" ← "going forward TypeScript"  → Supersedes
/// 3. new incumbent "going forward TypeScript" ← "going forward TypeScript"  → Same
///
/// This mirrors a real agent-memory scenario where a user updates their
/// preferred language twice; the engine correctly classifies each step.
#[test]
fn multi_hop_belief_chain_resolved_to_canonical() {
    // Step 1: Rust → superseded by Python.
    let step1 = SemanticArbitrationEngine::arbitrate(
        &ArbitrationInput::new("Rust", "actually Python now", "project", "language"),
        None,
        None,
    );
    assert_eq!(
        step1.resolution,
        BeliefConflictResolution::Supersedes,
        "step 1 should be Supersedes"
    );
    assert_eq!(step1.resolver_name, "rules");

    // Step 2: Python → superseded by TypeScript.
    let step2 = SemanticArbitrationEngine::arbitrate(
        &ArbitrationInput::new(
            "actually Python now",
            "going forward TypeScript",
            "project",
            "language",
        ),
        None,
        None,
    );
    assert_eq!(
        step2.resolution,
        BeliefConflictResolution::Supersedes,
        "step 2 should be Supersedes"
    );
    assert_eq!(step2.resolver_name, "rules");

    // Step 3: canonical value re-presented → Same.
    let step3 = SemanticArbitrationEngine::arbitrate(
        &ArbitrationInput::new(
            "going forward TypeScript",
            "going forward TypeScript",
            "project",
            "language",
        ),
        None,
        None,
    );
    assert_eq!(
        step3.resolution,
        BeliefConflictResolution::Same,
        "step 3 re-presenting the canonical value should be Same"
    );
    assert_eq!(step3.resolver_name, "rules");
}
