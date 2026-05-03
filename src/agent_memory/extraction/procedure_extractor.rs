use std::collections::BTreeMap;

use chrono::Utc;
use regex::Regex;
use uuid::Uuid;

use super::super::enums::{MemoryType, Scope, SourceType};
use super::super::schemas::{CandidateMemory, Provenance};
use super::entity_resolver::EntityResolver;

/// Extracts procedural / how-to sequences from plain text.
///
/// A procedure is detected when the text contains any of:
/// * `"how to"` phrase
/// * Numbered list (`1.`, `2.`, …; at least two steps)
/// * `"step 1"` / `"step 2"` markers
/// * `"first … then …"` pattern
pub struct ProcedureExtractor;

impl ProcedureExtractor {
    /// Extract zero or one procedure candidate from `text`.
    ///
    /// At most one candidate is returned because the full text is treated as a
    /// single procedure rather than splitting individual steps.
    #[must_use]
    pub fn extract(text: &str, resolver: &EntityResolver) -> Vec<CandidateMemory> {
        if !looks_like_procedure(text) {
            return Vec::new();
        }
        let entity = resolver.resolve_subject("I");
        let slot = infer_procedure_name(text);
        vec![CandidateMemory {
            candidate_id: Uuid::new_v4().to_string(),
            observed_at: Utc::now(),
            entity,
            slot: Some(slot),
            value: Some(text.to_string()),
            raw_text: text.to_string(),
            source: Provenance {
                source_type: SourceType::Chat,
                source_id: String::new(),
                source_label: None,
                observed_by: None,
                trust_weight: 1.0,
            },
            memory_type: MemoryType::Skill,
            confidence: 0.65,
            salience: 0.65,
            scope: Scope::Private,
            ttl: None,
            event_at: None,
            valid_from: None,
            valid_to: None,
            internal_layer: None,
            tags: vec!["procedure".to_string()],
            metadata: BTreeMap::new(),
            is_retraction: false,
        }]
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn looks_like_procedure(text: &str) -> bool {
    let lower = text.to_lowercase();

    // Explicit "how to" phrase.
    if lower.contains("how to") {
        return true;
    }

    // Numbered list: two or more `N.` or `N)` markers.
    let numbered_re = Regex::new(r"(?m)^\s*\d+[.)]\s").unwrap_or_else(|_| unreachable!());
    if numbered_re.find_iter(text).count() >= 2 {
        return true;
    }

    // Explicit step markers.
    if lower.contains("step 1") || lower.contains("step 2") {
        return true;
    }

    // "first … then …" sequence.
    lower.contains("first") && lower.contains("then")
}

/// Derive a short procedure name from the first sentence / "how to" phrase.
fn infer_procedure_name(text: &str) -> String {
    let lower = text.to_lowercase();
    if let Some(idx) = lower.find("how to") {
        // Grab up to 60 chars after "how to" as a natural name.
        let after = text[idx..].trim();
        let end = after
            .find(['.', '\n', '!', '?'])
            .unwrap_or_else(|| after.len().min(60));
        return after[..end].trim().to_string();
    }
    // Fall back: first sentence.
    text.split(['.', '\n', '!', '?'])
        .next()
        .unwrap_or(text)
        .trim()
        .chars()
        .take(60)
        .collect()
}
