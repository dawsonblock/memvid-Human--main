//! Typed semantic conflict arbitration for agent memory beliefs.
//!
//! [`SemanticArbitrationEngine`] is a stateless, three-pass orchestrator that
//! classifies the relationship between two candidate belief values:
//!
//! 1. **Rules pass** — fast, stateless, zero-cost.
//! 2. **Embedding pass** — cosine similarity (optional; requires an
//!    [`AgentEmbeddingProvider`] to be supplied).
//! 3. **LLM pass** — delegated to an optional [`LLMConflictArbiter`] backend.
//!
//! Each pass short-circuits on a non-[`BeliefConflictResolution::Ambiguous`]
//! result.  When all three passes are exhausted without resolution the engine
//! returns `Ambiguous` with `resolver_name = "fallback"`.
//!
//! [`ConflictArbiter`](super::conflict_resolution::ConflictArbiter) delegates
//! to this engine, keeping its existing `ConflictResolutionTrace` API intact
//! for backward compatibility.

use super::belief_conflict_resolver::{
    BeliefConflictResolution, BeliefConflictResolver, ConflictContext,
};
use super::embedding_provider::AgentEmbeddingProvider;
use super::enums::SourceType;
use super::errors::Result;

// ── Cosine-similarity decision thresholds ────────────────────────────────────

/// Cosine similarity above which values are treated as semantically identical.
const COSINE_SAME: f32 = 0.92;
/// Cosine similarity above which values are treated as reinforcing.
const COSINE_REINFORCE: f32 = 0.78;
/// Cosine similarity below which values are treated as opposing / contradictory.
const COSINE_CONTRADICT: f32 = 0.28;

// ── LLM arbiter trait ────────────────────────────────────────────────────────

/// Optional LLM backend for resolving conflicts that remain ambiguous after
/// rule-based and embedding passes.
///
/// Returning [`BeliefConflictResolution::Ambiguous`] is valid and signals
/// that the LLM could not determine a concrete resolution.
pub trait LLMConflictArbiter: Send + Sync {
    /// Ask the LLM to classify the relationship between `existing` and
    /// `incoming` values for the same (entity, slot) belief.
    fn arbitrate(
        &self,
        existing: &str,
        incoming: &str,
        context: &ConflictContext,
    ) -> Result<BeliefConflictResolution>;
}

// ── Input / output types ─────────────────────────────────────────────────────

/// Structured inputs for semantic arbitration.
#[derive(Debug, Clone)]
pub struct ArbitrationInput {
    /// Free-form natural language context about the arbitration scenario.
    pub context: String,
    /// First candidate value (the existing / incumbent belief value).
    pub candidate_a: String,
    /// Second candidate value (the incoming / challenger belief value).
    pub candidate_b: String,
    /// Entity the belief belongs to.
    pub entity: String,
    /// Slot (attribute) name within `entity`.
    pub slot: String,
}

impl ArbitrationInput {
    /// Convenience constructor.
    pub fn new(
        candidate_a: impl Into<String>,
        candidate_b: impl Into<String>,
        entity: impl Into<String>,
        slot: impl Into<String>,
    ) -> Self {
        let a = candidate_a.into();
        let b = candidate_b.into();
        let e = entity.into();
        let s = slot.into();
        ArbitrationInput {
            context: format!("belief update for {e}.{s}"),
            candidate_a: a,
            candidate_b: b,
            entity: e,
            slot: s,
        }
    }
}

/// Outcome of semantic arbitration.
#[derive(Debug, Clone)]
pub struct ArbitrationOutcome {
    /// Classified relationship between the two candidates.
    pub resolution: BeliefConflictResolution,
    /// Normalised confidence in the resolution (0.0 – 1.0).
    pub confidence: f32,
    /// Name of the resolver that produced this outcome.
    /// One of `"rules"`, `"embedding"`, `"llm"`, or `"fallback"`.
    pub resolver_name: &'static str,
    /// Human-readable rationale string.
    pub rationale: String,
    /// Cosine similarity computed during the embedding pass, when applicable.
    pub cosine_similarity: Option<f32>,
}

// ── SemanticArbitrationEngine ─────────────────────────────────────────────────

/// Stateless three-pass semantic arbitration engine.
///
/// Run [`SemanticArbitrationEngine::arbitrate`] directly — the struct carries
/// no state.
#[derive(Debug, Default, Clone, Copy)]
pub struct SemanticArbitrationEngine;

impl SemanticArbitrationEngine {
    /// Classify the relationship between `input.candidate_a` (existing) and
    /// `input.candidate_b` (incoming) using an ordered chain of resolvers.
    ///
    /// # Pass order
    ///
    /// 1. **Rules** — always executed.
    /// 2. **Embedding** — executed when `embedder` is `Some(…)`.
    /// 3. **LLM** — executed when `llm` is `Some(…)`.
    ///
    /// Each pass is skipped entirely when the previous pass returned a
    /// non-`Ambiguous` verdict.
    pub fn arbitrate(
        input: &ArbitrationInput,
        embedder: Option<&dyn AgentEmbeddingProvider>,
        llm: Option<&dyn LLMConflictArbiter>,
    ) -> ArbitrationOutcome {
        // Build a ConflictContext.
        // `BeliefConflictResolver` ignores `source_type`; default is safe here.
        let conflict_ctx = ConflictContext {
            existing: input.candidate_a.clone(),
            incoming: input.candidate_b.clone(),
            source_type: SourceType::Chat,
        };

        // ── Pass 1: rule-based ────────────────────────────────────────────
        let rule_resolution =
            BeliefConflictResolver::resolve(&input.candidate_a, &input.candidate_b, &conflict_ctx);
        if rule_resolution != BeliefConflictResolution::Ambiguous {
            return ArbitrationOutcome {
                rationale: format!(
                    "Rule-based arbiter resolved ({}, {}) as {:?}",
                    input.entity, input.slot, rule_resolution
                ),
                confidence: 0.95,
                resolver_name: "rules",
                resolution: rule_resolution,
                cosine_similarity: None,
            };
        }

        // ── Pass 2: embedding cosine ──────────────────────────────────────
        if let Some(provider) = embedder {
            let embed_result = (
                provider.embed(&input.candidate_a),
                provider.embed(&input.candidate_b),
            );
            if let (Ok(a), Ok(b)) = embed_result {
                use super::embedding_provider::cosine;
                let sim = cosine(&a, &b);
                let embed_resolution = embedding_resolution(sim);
                if embed_resolution != BeliefConflictResolution::Ambiguous {
                    return ArbitrationOutcome {
                        rationale: format!(
                            "Embedding cosine {:.3} resolved ({}, {}) as {:?}",
                            sim, input.entity, input.slot, embed_resolution
                        ),
                        confidence: sim.abs().clamp(0.50, 0.95),
                        resolver_name: "embedding",
                        resolution: embed_resolution,
                        cosine_similarity: Some(sim),
                    };
                }
                // Embedding ambiguous — try LLM with cosine already computed.
                if let Some(llm_arbiter) = llm {
                    if let Ok(llm_res) =
                        llm_arbiter.arbitrate(&input.candidate_a, &input.candidate_b, &conflict_ctx)
                    {
                        if llm_res != BeliefConflictResolution::Ambiguous {
                            return ArbitrationOutcome {
                                rationale: format!(
                                    "LLM arbiter (cosine={:.3}) resolved ({}, {}) as {:?}",
                                    sim, input.entity, input.slot, llm_res
                                ),
                                confidence: 0.85,
                                resolver_name: "llm",
                                resolution: llm_res,
                                cosine_similarity: Some(sim),
                            };
                        }
                    }
                }
                // Still ambiguous — return with cosine score for observability.
                return ArbitrationOutcome {
                    rationale: format!(
                        "All passes ambiguous for ({}, {}); cosine={:.3}",
                        input.entity, input.slot, sim
                    ),
                    confidence: 0.30,
                    resolver_name: "fallback",
                    resolution: BeliefConflictResolution::Ambiguous,
                    cosine_similarity: Some(sim),
                };
            }
        }

        // ── Pass 3: LLM arbiter (embedder absent or embed failed) ─────────
        if let Some(llm_arbiter) = llm {
            if let Ok(llm_res) =
                llm_arbiter.arbitrate(&input.candidate_a, &input.candidate_b, &conflict_ctx)
            {
                if llm_res != BeliefConflictResolution::Ambiguous {
                    return ArbitrationOutcome {
                        rationale: format!(
                            "LLM arbiter resolved ({}, {}) as {:?}",
                            input.entity, input.slot, llm_res
                        ),
                        confidence: 0.85,
                        resolver_name: "llm",
                        resolution: llm_res,
                        cosine_similarity: None,
                    };
                }
            }
        }

        // ── Fallback: all passes exhausted without resolution ─────────────
        ArbitrationOutcome {
            rationale: format!(
                "All passes ambiguous for ({}, {})",
                input.entity, input.slot
            ),
            confidence: 0.30,
            resolver_name: "fallback",
            resolution: BeliefConflictResolution::Ambiguous,
            cosine_similarity: None,
        }
    }
}

// ── Internal helper ───────────────────────────────────────────────────────────

/// Map a cosine similarity value to a [`BeliefConflictResolution`].
///
/// Returns [`BeliefConflictResolution::Ambiguous`] when the similarity falls
/// in the uncertain mid-range ([`COSINE_CONTRADICT`] … [`COSINE_REINFORCE`]).
fn embedding_resolution(sim: f32) -> BeliefConflictResolution {
    if sim > COSINE_SAME {
        BeliefConflictResolution::Same
    } else if sim > COSINE_REINFORCE {
        BeliefConflictResolution::Reinforces
    } else if sim < COSINE_CONTRADICT {
        BeliefConflictResolution::Contradicts
    } else {
        BeliefConflictResolution::Ambiguous
    }
}
