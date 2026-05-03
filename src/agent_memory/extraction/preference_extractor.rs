use std::collections::BTreeMap;

use chrono::Utc;
use uuid::Uuid;

use super::super::enums::{MemoryType, Scope, SourceType};
use super::super::schemas::{CandidateMemory, Provenance};
use super::entity_resolver::EntityResolver;

/// Preference signal patterns: (lowercase trigger phrase, slot label).
static PREFERENCE_PATTERNS: &[(&str, &str)] = &[
    ("i prefer ", "prefers"),
    ("i like ", "likes"),
    ("i love ", "loves"),
    ("i hate ", "dislikes"),
    ("i dislike ", "dislikes"),
    ("i want ", "wants"),
    ("i don't like ", "dislikes"),
    ("i do not like ", "dislikes"),
    ("i enjoy ", "likes"),
    ("i can't stand ", "dislikes"),
];

/// Extracts first-person preference statements from plain text.
pub struct PreferenceExtractor;

impl PreferenceExtractor {
    /// Extract zero or more preference candidates from `text`.
    #[must_use]
    pub fn extract(text: &str, resolver: &EntityResolver) -> Vec<CandidateMemory> {
        let mut results: Vec<CandidateMemory> = Vec::new();
        let lower = text.to_lowercase();

        for (pattern, slot) in PREFERENCE_PATTERNS {
            let mut search_from = 0usize;
            while let Some(rel_pos) = lower[search_from..].find(pattern) {
                let abs_pos = search_from + rel_pos;
                let value_start = abs_pos + pattern.len();
                if value_start >= text.len() {
                    search_from = abs_pos + 1;
                    continue;
                }
                // Value runs until the next sentence terminator.
                let value = text[value_start..]
                    .split(['.', ',', '!', '?', '\n', ';'])
                    .next()
                    .unwrap_or("")
                    .trim();
                if !value.is_empty() {
                    // First-person preference: entity is the speaker, resolved from hint.
                    let entity = resolver.resolve_subject("I");
                    results.push(CandidateMemory {
                        candidate_id: Uuid::new_v4().to_string(),
                        observed_at: Utc::now(),
                        entity,
                        slot: Some((*slot).to_string()),
                        value: Some(value.to_string()),
                        raw_text: text.to_string(),
                        source: Provenance {
                            source_type: SourceType::Chat,
                            source_id: String::new(),
                            source_label: None,
                            observed_by: None,
                            trust_weight: 1.0,
                        },
                        memory_type: MemoryType::Preference,
                        confidence: 0.75,
                        salience: 0.70,
                        scope: Scope::Private,
                        ttl: None,
                        event_at: None,
                        valid_from: None,
                        valid_to: None,
                        internal_layer: None,
                        tags: Vec::new(),
                        metadata: BTreeMap::new(),
                        is_retraction: false,
                    });
                }
                search_from = abs_pos + 1;
            }
        }
        results
    }
}
