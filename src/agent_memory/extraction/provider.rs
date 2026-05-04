use std::collections::HashMap;

use super::super::schemas::{CandidateMemory, IngestContext};

// ─── Provider trait ──────────────────────────────────────────────────────────

/// Pluggable extraction provider.
///
/// Implement this trait to add new extraction heuristics or structured extractors
/// (e.g. LLM-backed JSON extractors) without modifying the core pipeline.  All
/// providers are run in sequence during [`RawInputProcessor::process`] and their
/// results are merged through [`MergedExtractionValidator`] before being returned.
pub trait MemoryExtractionProvider: Send + Sync {
    /// Attempt to extract zero or more [`CandidateMemory`] items from `text`.
    ///
    /// `context` carries caller-supplied hints (entity, scope, tags …).
    /// Implementations must never panic; return an empty vec instead.
    fn extract(&self, text: &str, context: &IngestContext) -> Vec<CandidateMemory>;

    /// Stable human-readable identifier used in logs and debug output.
    fn name(&self) -> &'static str;
}

// ─── Deduplication validator ─────────────────────────────────────────────────

/// Deduplicates candidates produced by multiple providers.
///
/// Two candidates are considered identical when they share the same
/// `(entity, slot, value)` triple (all lowercased and trimmed).  For each
/// duplicate group the candidate with the highest `confidence` score is
/// kept; the others are discarded.
#[derive(Debug, Default, Clone)]
pub struct MergedExtractionValidator;

/// Opaque deduplication key derived from `(entity, slot, value)`.
fn dedup_key(c: &CandidateMemory) -> String {
    let entity = c.entity.as_deref().unwrap_or("").trim().to_lowercase();
    let slot = c.slot.as_deref().unwrap_or("").trim().to_lowercase();
    let value = c.value.as_deref().unwrap_or("").trim().to_lowercase();
    format!("{entity}\x00{slot}\x00{value}")
}

impl MergedExtractionValidator {
    /// Deduplicate `candidates` and return the merged list.
    ///
    /// Candidates with no entity/slot/value at all (generic traces) are always
    /// kept because their raw text is unique.
    #[must_use]
    pub fn deduplicate(&self, candidates: Vec<CandidateMemory>) -> Vec<CandidateMemory> {
        // Separate generic traces (entity + slot + value all None/empty) from
        // structured candidates so they are never accidentally collapsed.
        let mut generic_traces: Vec<CandidateMemory> = Vec::new();
        let mut structured: Vec<CandidateMemory> = Vec::new();

        for c in candidates {
            if c.entity.is_none() && c.slot.is_none() && c.value.is_none() {
                generic_traces.push(c);
            } else {
                structured.push(c);
            }
        }

        // For each dedup key, keep the highest-confidence candidate.
        let mut best: HashMap<String, CandidateMemory> = HashMap::new();
        for c in structured {
            let key = dedup_key(&c);
            best.entry(key)
                .and_modify(|existing| {
                    if c.confidence > existing.confidence {
                        *existing = c.clone();
                    }
                })
                .or_insert(c);
        }

        let mut result: Vec<CandidateMemory> = best.into_values().collect();
        result.extend(generic_traces);
        result
    }
}

// ─── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use uuid::Uuid;

    use super::super::super::enums::{MemoryType, Scope, SourceType};
    use super::super::super::schemas::Provenance;
    use super::*;

    fn make_candidate(
        entity: Option<&str>,
        slot: Option<&str>,
        value: Option<&str>,
        confidence: f32,
    ) -> CandidateMemory {
        CandidateMemory {
            candidate_id: Uuid::new_v4().to_string(),
            observed_at: Utc::now(),
            entity: entity.map(str::to_string),
            slot: slot.map(str::to_string),
            value: value.map(str::to_string),
            raw_text: "test".to_string(),
            source: Provenance {
                source_type: SourceType::Chat,
                source_id: String::new(),
                source_label: None,
                observed_by: None,
                trust_weight: 1.0,
            },
            memory_type: MemoryType::Fact,
            confidence,
            salience: 0.5,
            scope: Scope::Private,
            ttl: None,
            event_at: None,
            valid_from: None,
            valid_to: None,
            internal_layer: None,
            tags: Vec::new(),
            metadata: BTreeMap::new(),
            is_retraction: false,
            parent_memory_id: None,
            thread_id: None,
        }
    }

    #[test]
    fn dedup_keeps_higher_confidence() {
        let validator = MergedExtractionValidator;
        let c1 = make_candidate(Some("alice"), Some("likes"), Some("cats"), 0.6);
        let c2 = make_candidate(Some("alice"), Some("likes"), Some("cats"), 0.9);
        let result = validator.deduplicate(vec![c1, c2]);
        assert_eq!(result.len(), 1);
        assert!((result[0].confidence - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn dedup_keeps_distinct_keys() {
        let validator = MergedExtractionValidator;
        let c1 = make_candidate(Some("alice"), Some("likes"), Some("cats"), 0.7);
        let c2 = make_candidate(Some("alice"), Some("likes"), Some("dogs"), 0.7);
        let result = validator.deduplicate(vec![c1, c2]);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn generic_traces_never_deduplicated() {
        let validator = MergedExtractionValidator;
        let t1 = make_candidate(None, None, None, 0.3);
        let t2 = make_candidate(None, None, None, 0.3);
        let result = validator.deduplicate(vec![t1, t2]);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn case_insensitive_dedup() {
        let validator = MergedExtractionValidator;
        let c1 = make_candidate(Some("Alice"), Some("Likes"), Some("Cats"), 0.5);
        let c2 = make_candidate(Some("alice"), Some("likes"), Some("cats"), 0.8);
        let result = validator.deduplicate(vec![c1, c2]);
        assert_eq!(result.len(), 1);
        assert!((result[0].confidence - 0.8).abs() < f32::EPSILON);
    }
}
