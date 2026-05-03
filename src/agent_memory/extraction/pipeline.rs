use chrono::Utc;
use uuid::Uuid;

use super::super::enums::MemoryType;
use super::super::schemas::{CandidateMemory, IngestContext, Provenance};
use super::candidate_scorer::CandidateScorer;
use super::claim_extractor::ClaimExtractor;
use super::entity_resolver::EntityResolver;
use super::preference_extractor::PreferenceExtractor;
use super::procedure_extractor::ProcedureExtractor;
use super::temporal_normalizer::TemporalNormalizer;

/// Holds the outcome of processing one raw text input.
#[derive(Debug, Clone)]
pub struct ExtractionResult {
    pub candidates: Vec<CandidateMemory>,
    pub raw_text: String,
}

/// Orchestrates all sub-extractors to convert raw text into `CandidateMemory` items.
#[derive(Debug, Default, Clone)]
pub struct RawInputProcessor;

impl RawInputProcessor {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Parse `text` and return zero or more `CandidateMemory` candidates ready
    /// for classification and promotion.
    pub fn process(&self, text: &str, context: &IngestContext) -> Vec<CandidateMemory> {
        let resolver = EntityResolver::new(context.entity_hint.clone());
        let scorer = CandidateScorer::default();

        let mut candidates: Vec<CandidateMemory> = Vec::new();

        // 1. Preferences ("I prefer/like/hate …")
        let prefs = PreferenceExtractor::extract(text, &resolver);
        candidates.extend(prefs);

        // 2. Procedure / how-to steps
        let steps = ProcedureExtractor::extract(text, &resolver);
        candidates.extend(steps);

        // 3. SVO fact claims ("X is Y", "X has Y", "X: Y")
        let claims = ClaimExtractor::extract(text, &resolver);
        candidates.extend(claims);

        // If no sub-extractor fired, emit a single generic trace candidate so the
        // caller always has something to route through the policy pipeline.
        if candidates.is_empty() {
            candidates.push(generic_trace(text, context));
        }

        // Apply temporal normalisation and scoring to all candidates.
        let normalizer = TemporalNormalizer::default();
        for c in &mut candidates {
            normalizer.normalize(c);
            scorer.score(c);
            // Stamp provenance from context.
            c.source.source_type = context.source_type;
            c.scope = context.scope;
            c.tags.extend(context.tags.clone());
        }

        candidates
    }
}

fn generic_trace(text: &str, context: &IngestContext) -> CandidateMemory {
    CandidateMemory {
        candidate_id: Uuid::new_v4().to_string(),
        observed_at: Utc::now(),
        entity: None,
        slot: None,
        value: None,
        raw_text: text.to_string(),
        source: Provenance {
            source_type: context.source_type,
            source_id: String::new(),
            source_label: None,
            observed_by: None,
            trust_weight: 1.0,
        },
        memory_type: MemoryType::Trace,
        confidence: 0.3,
        salience: 0.2,
        scope: context.scope,
        ttl: None,
        event_at: None,
        valid_from: None,
        valid_to: None,
        internal_layer: None,
        tags: context.tags.clone(),
        metadata: context.metadata.clone(),
        is_retraction: false,
    }
}
