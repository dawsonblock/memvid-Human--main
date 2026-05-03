use std::collections::BTreeMap;

use chrono::Utc;
use regex::Regex;
use uuid::Uuid;

use super::super::enums::{MemoryType, Scope, SourceType};
use super::super::schemas::{CandidateMemory, Provenance};
use super::entity_resolver::EntityResolver;

/// Extracts factual SVO-style claims from plain text.
///
/// Recognised patterns:
/// * `"X is Y"` / `"X are Y"`
/// * `"X has Y"`
/// * `"label: value"` (single-word labels only)
pub struct ClaimExtractor;

impl ClaimExtractor {
    /// Extract zero or more fact candidates from `text`.
    #[must_use]
    pub fn extract(text: &str, resolver: &EntityResolver) -> Vec<CandidateMemory> {
        let mut results: Vec<CandidateMemory> = Vec::new();

        // Split into sentences on common terminators.
        for sentence in text.split(['.', '!', '?', '\n']) {
            let s = sentence.trim();
            if s.is_empty() {
                continue;
            }
            if let Some(c) = try_is_are(s, resolver) {
                results.push(c);
                continue;
            }
            if let Some(c) = try_has(s, resolver) {
                results.push(c);
                continue;
            }
            if let Some(c) = try_colon(s, resolver) {
                results.push(c);
            }
        }
        results
    }
}

// ─── pattern helpers ─────────────────────────────────────────────────────────

fn try_is_are(sentence: &str, resolver: &EntityResolver) -> Option<CandidateMemory> {
    // Matches "SUBJECT is|are PREDICATE" — word-boundary aware, case-insensitive.
    let re = Regex::new(r"(?i)^(.+?)\b(is|are)\b(.+)$").ok()?;
    let caps = re.captures(sentence)?;
    let subject = caps.get(1)?.as_str().trim();
    let verb = caps.get(2)?.as_str().trim();
    let predicate = caps.get(3)?.as_str().trim();
    if subject.is_empty() || predicate.is_empty() {
        return None;
    }
    Some(make_fact(
        resolver.resolve_subject(subject),
        verb,
        predicate,
        sentence,
    ))
}

fn try_has(sentence: &str, resolver: &EntityResolver) -> Option<CandidateMemory> {
    let re = Regex::new(r"(?i)^(.+?)\bhas\b(.+)$").ok()?;
    let caps = re.captures(sentence)?;
    let subject = caps.get(1)?.as_str().trim();
    let predicate = caps.get(2)?.as_str().trim();
    if subject.is_empty() || predicate.is_empty() {
        return None;
    }
    Some(make_fact(
        resolver.resolve_subject(subject),
        "has",
        predicate,
        sentence,
    ))
}

fn try_colon(sentence: &str, resolver: &EntityResolver) -> Option<CandidateMemory> {
    // "label: value" — label must be a single word (no internal spaces).
    let re = Regex::new(r"^(\S+):\s+(.+)$").ok()?;
    let caps = re.captures(sentence)?;
    let label = caps.get(1)?.as_str().trim();
    let value = caps.get(2)?.as_str().trim();
    if label.is_empty() || value.is_empty() {
        return None;
    }
    Some(make_fact(
        resolver.resolve_subject(label),
        "value",
        value,
        sentence,
    ))
}

fn make_fact(entity: Option<String>, slot: &str, value: &str, raw: &str) -> CandidateMemory {
    CandidateMemory {
        candidate_id: Uuid::new_v4().to_string(),
        observed_at: Utc::now(),
        entity,
        slot: Some(slot.to_string()),
        value: Some(value.to_string()),
        raw_text: raw.to_string(),
        source: Provenance {
            source_type: SourceType::Chat,
            source_id: String::new(),
            source_label: None,
            observed_by: None,
            trust_weight: 1.0,
        },
        memory_type: MemoryType::Fact,
        confidence: 0.5,
        salience: 0.4,
        scope: Scope::Private,
        ttl: None,
        event_at: None,
        valid_from: None,
        valid_to: None,
        internal_layer: None,
        tags: Vec::new(),
        metadata: BTreeMap::new(),
        is_retraction: false,
    }
}
