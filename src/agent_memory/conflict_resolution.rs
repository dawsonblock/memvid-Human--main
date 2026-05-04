//! Chained semantic conflict arbitration for agent memory beliefs.
//!
//! The [`ConflictArbiter`] runs up to three passes to classify the
//! relationship between an existing belief value and an incoming one:
//!
//! 1. **Rules pass** — fast, stateless, zero-cost. Delegates to
//!    [`BeliefConflictResolver::resolve`]. Returns immediately on any
//!    non-[`BeliefConflictResolution::Ambiguous`] result.
//!
//! 2. **Embedding pass** — uses cosine similarity if an
//!    [`AgentEmbeddingProvider`] is supplied. Cosine bands map to
//!    concrete resolutions (see constants below). Only executed when
//!    `embedder` is `Some(…)`.
//!
//! 3. **LLM pass** — delegates to an optional [`LLMConflictArbiter`]
//!    implementation (trait only — no HTTP client is bundled here).
//!    Only executed when `llm` is `Some(…)`.
//!
//! If all three passes yield [`BeliefConflictResolution::Ambiguous`] the
//! arbiter returns that result with `resolver_name = "fallback"` so that
//! callers can apply trust-weighted fallback logic.

use super::belief_conflict_resolver::{
    BeliefConflictResolution, BeliefConflictResolver, ConflictContext,
};
use super::embedding_provider::AgentEmbeddingProvider;
use super::errors::Result;

// ── Cosine-similarity decision thresholds ────────────────────────────────────

/// Cosine similarity above which values are treated as semantically identical.
const COSINE_SAME: f32 = 0.92;
/// Cosine similarity above which values are treated as reinforcing.
const COSINE_REINFORCE: f32 = 0.78;
/// Cosine similarity below which values are treated as opposing / contradictory.
const COSINE_CONTRADICT: f32 = 0.28;

// ── Resolution trace ─────────────────────────────────────────────────────────

/// Enriched outcome of [`ConflictArbiter::resolve`].
///
/// Carries the concrete resolution *plus* provenance information so that
/// callers (and tests) can inspect which resolver produced the result.
#[derive(Debug, Clone)]
pub struct ConflictResolutionTrace {
    /// Final classification.
    pub resolution: BeliefConflictResolution,
    /// Name of the resolver that produced `resolution`.
    /// One of `"rules"`, `"embedding"`, `"llm"`, or `"fallback"`.
    pub resolver_name: &'static str,
    /// Cosine similarity between the two value embeddings, when computed.
    pub cosine_similarity: Option<f32>,
    /// `true` when the LLM arbiter was consulted *and* returned a
    /// non-Ambiguous result.
    pub llm_used: bool,
}

// ── LLM arbiter trait ────────────────────────────────────────────────────────

/// Optional LLM backend for resolving conflicts that are ambiguous to both
/// the rule-based and embedding resolvers.
///
/// This is strictly a core trait — no HTTP implementation is provided by
/// default.  Supply a concrete type via [`ConflictArbiter::resolve`]'s
/// `llm` parameter to enable LLM-backed arbitration.
pub trait LLMConflictArbiter: Send + Sync {
    /// Ask the LLM to classify the relationship between `existing` and
    /// `incoming` values (same (entity, slot) belief).
    ///
    /// Returning [`BeliefConflictResolution::Ambiguous`] is valid and
    /// signals that the LLM could not determine a concrete resolution.
    fn arbitrate(
        &self,
        existing: &str,
        incoming: &str,
        context: &ConflictContext,
    ) -> Result<BeliefConflictResolution>;
}

// ── ConflictArbiter ───────────────────────────────────────────────────────────

/// Stateless, three-pass conflict arbiter.
///
/// Instantiate (zero-cost — it carries no state) and call
/// [`ConflictArbiter::resolve`] directly.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConflictArbiter;

impl ConflictArbiter {
    /// Classify the relationship between `existing` and `incoming` belief
    /// values using an ordered chain of resolvers.
    ///
    /// # Pass order
    ///
    /// 1. **Rules** — always executed.
    /// 2. **Embedding** — executed when `embedder` is `Some(…)`.
    /// 3. **LLM** — executed when `llm` is `Some(…)`.
    ///
    /// Each pass is skipped entirely (not just its result discarded) if the
    /// previous pass returned a non-Ambiguous verdict.
    ///
    /// # Arguments
    ///
    /// * `existing`  — current belief value string.
    /// * `incoming`  — candidate new value string.
    /// * `context`   — additional signals for rule-based resolution.
    /// * `embedder`  — optional dense-embedding provider.
    /// * `llm`       — optional LLM conflict arbiter.
    pub fn resolve(
        existing: &str,
        incoming: &str,
        context: &ConflictContext,
        embedder: Option<&dyn AgentEmbeddingProvider>,
        llm: Option<&dyn LLMConflictArbiter>,
    ) -> ConflictResolutionTrace {
        // ── Pass 1: rule-based ────────────────────────────────────────────
        let rule_resolution = BeliefConflictResolver::resolve(existing, incoming, context);
        if rule_resolution != BeliefConflictResolution::Ambiguous {
            return ConflictResolutionTrace {
                resolution: rule_resolution,
                resolver_name: "rules",
                cosine_similarity: None,
                llm_used: false,
            };
        }

        // ── Pass 2: embedding cosine ──────────────────────────────────────
        if let Some(provider) = embedder {
            let embed_result = (provider.embed(existing), provider.embed(incoming));
            if let (Ok(a), Ok(b)) = embed_result {
                use super::embedding_provider::cosine;
                let sim = cosine(&a, &b);
                let embed_resolution = embedding_resolution(sim);
                if embed_resolution != BeliefConflictResolution::Ambiguous {
                    return ConflictResolutionTrace {
                        resolution: embed_resolution,
                        resolver_name: "embedding",
                        cosine_similarity: Some(sim),
                        llm_used: false,
                    };
                }
                // Ambiguous after embeddings — fall through to LLM with
                // the cosine similarity already computed.
                if let Some(llm_arbiter) = llm {
                    if let Ok(llm_resolution) = llm_arbiter.arbitrate(existing, incoming, context) {
                        if llm_resolution != BeliefConflictResolution::Ambiguous {
                            return ConflictResolutionTrace {
                                resolution: llm_resolution,
                                resolver_name: "llm",
                                cosine_similarity: Some(sim),
                                llm_used: true,
                            };
                        }
                    }
                }
                // Still ambiguous — return with the cosine score for observability.
                return ConflictResolutionTrace {
                    resolution: BeliefConflictResolution::Ambiguous,
                    resolver_name: "fallback",
                    cosine_similarity: Some(sim),
                    llm_used: false,
                };
            }
        }

        // ── Pass 3: LLM arbiter (embedder absent or embed failed) ─────────
        if let Some(llm_arbiter) = llm {
            if let Ok(llm_resolution) = llm_arbiter.arbitrate(existing, incoming, context) {
                if llm_resolution != BeliefConflictResolution::Ambiguous {
                    return ConflictResolutionTrace {
                        resolution: llm_resolution,
                        resolver_name: "llm",
                        cosine_similarity: None,
                        llm_used: true,
                    };
                }
            }
        }

        // ── Fallback: still Ambiguous ─────────────────────────────────────
        ConflictResolutionTrace {
            resolution: BeliefConflictResolution::Ambiguous,
            resolver_name: "fallback",
            cosine_similarity: None,
            llm_used: false,
        }
    }
}

// ── Internal helper ───────────────────────────────────────────────────────────

/// Map a cosine similarity value to a [`BeliefConflictResolution`].
///
/// Returns [`BeliefConflictResolution::Ambiguous`] when the similarity
/// falls in the uncertain region ([`COSINE_CONTRADICT`] … [`COSINE_REINFORCE`]).
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
