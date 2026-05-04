//! Backward-compatible conflict arbitration facade.
//!
//! [`ConflictArbiter`] preserves the original public API while delegating
//! all three-pass logic to [`SemanticArbitrationEngine`], which now owns the
//! canonical implementation.  Callers that already use
//! `ConflictArbiter::resolve` do not need to change.
//!
//! [`LLMConflictArbiter`] is re-exported from
//! [`super::semantic_arbitration`] for backward compatibility.

use super::belief_conflict_resolver::{BeliefConflictResolution, ConflictContext};
use super::embedding_provider::AgentEmbeddingProvider;
use super::semantic_arbitration::{ArbitrationInput, SemanticArbitrationEngine};

/// Re-exported from [`super::semantic_arbitration`] for backward compatibility.
pub use super::semantic_arbitration::LLMConflictArbiter;

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
        let input = ArbitrationInput {
            context: format!("{} vs {}", context.existing, context.incoming),
            candidate_a: existing.to_string(),
            candidate_b: incoming.to_string(),
            // entity/slot are not available at this call site; leave empty
            entity: String::new(),
            slot: String::new(),
        };
        let outcome = SemanticArbitrationEngine::arbitrate(&input, embedder, llm);
        ConflictResolutionTrace {
            resolution: outcome.resolution,
            resolver_name: outcome.resolver_name,
            cosine_similarity: outcome.cosine_similarity,
            llm_used: outcome.resolver_name == "llm",
        }
    }
}
