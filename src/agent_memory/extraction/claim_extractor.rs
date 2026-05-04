use std::collections::BTreeMap;

use chrono::Utc;
use regex::Regex;
use uuid::Uuid;

use super::super::enums::{MemoryLayer, MemoryType, Scope, SourceType};
use super::super::schemas::{CandidateMemory, Provenance};
use super::entity_resolver::EntityResolver;

/// Extracts factual SVO-style claims and implicit signals from plain text.
///
/// Recognised patterns:
/// * `"X is Y"` / `"X are Y"` — direct fact
/// * `"X has Y"` — possession fact
/// * `"label: value"` — colon-delimited fact (single-word label only)
/// * `"from now on …"` / `"always …"` / `"remember to …"` — instruction hint
/// * `"don't …"` / `"never …"` / `"stop …"` / `"avoid …"` — negation/constraint
/// * `"unless …"` / `"except when …"` — conditional qualifier
/// * `"I changed my mind"` / `"actually …"` / `"that was wrong"` — self-correction
pub struct ClaimExtractor;

impl ClaimExtractor {
    /// Extract zero or more candidates from `text`.
    ///
    /// Each sentence is tested against all pattern groups in priority order.
    /// Instruction patterns take precedence; general SVO patterns are tried last.
    #[must_use]
    pub fn extract(text: &str, resolver: &EntityResolver) -> Vec<CandidateMemory> {
        let mut results: Vec<CandidateMemory> = Vec::new();

        // Split into sentences on common terminators.
        for sentence in text.split(['.', '!', '?', '\n']) {
            let s = sentence.trim();
            if s.is_empty() {
                continue;
            }

            // Self-correction signals take top priority — they indicate the prior
            // memory state should be re-evaluated.
            if let Some(c) = try_self_correction(s, resolver) {
                results.push(c);
                continue;
            }

            // Instruction patterns route to SelfModel layer.
            if let Some(c) = try_instruction(s, resolver) {
                results.push(c);
                continue;
            }

            // Negation/constraint patterns mark is_retraction = true.
            if let Some(c) = try_negation_constraint(s, resolver) {
                results.push(c);
                continue;
            }

            // Conditional qualifier — stored as metadata["condition"].
            if let Some(c) = try_conditional(s, resolver) {
                results.push(c);
                continue;
            }

            // General SVO / colon-delimited facts.
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
        thread_id: None,
        parent_memory_id: None,
    }
}

// ─── New pattern group helpers ────────────────────────────────────────────────

/// Instruction patterns: "from now on X", "always X", "remember to X",
/// "going forward X", "make sure you X".
///
/// These are routed to [`MemoryLayer::SelfModel`] because they express a
/// persistent directive about how the agent should behave, not a factual claim.
fn try_instruction(sentence: &str, resolver: &EntityResolver) -> Option<CandidateMemory> {
    let lower = sentence.to_lowercase();
    // Ordered list of (trigger_prefix, captured_value_start_offset).
    let triggers: &[&str] = &[
        "from now on ",
        "always ",
        "remember to ",
        "going forward ",
        "make sure you ",
        "please always ",
        "please remember to ",
    ];

    for trigger in triggers {
        if let Some(pos) = lower.find(trigger) {
            let value_start = pos + trigger.len();
            let value = sentence[value_start..].trim();
            if value.is_empty() {
                continue;
            }
            let entity = resolver.resolve_subject("instruction");
            let mut meta = BTreeMap::new();
            meta.insert(
                "instruction_trigger".to_string(),
                (*trigger).trim().to_string(),
            );
            return Some(CandidateMemory {
                candidate_id: Uuid::new_v4().to_string(),
                observed_at: Utc::now(),
                entity,
                slot: Some("instruction".to_string()),
                value: Some(value.to_string()),
                raw_text: sentence.to_string(),
                source: Provenance {
                    source_type: SourceType::Chat,
                    source_id: String::new(),
                    source_label: None,
                    observed_by: None,
                    trust_weight: 1.0,
                },
                memory_type: MemoryType::Fact,
                confidence: 0.70,
                salience: 0.65,
                scope: Scope::Private,
                ttl: None,
                event_at: None,
                valid_from: None,
                valid_to: None,
                internal_layer: Some(MemoryLayer::SelfModel),
                tags: vec!["instruction".to_string()],
                metadata: meta,
                is_retraction: false,
                thread_id: None,
                parent_memory_id: None,
            });
        }
    }
    None
}

/// Negation / constraint patterns: "don't X", "never X", "stop X", "avoid X",
/// "must not X", "do not X".
///
/// Sets `is_retraction = true` so the belief update pipeline knows this
/// candidate should suppress or retract a prior positive assertion.
fn try_negation_constraint(sentence: &str, resolver: &EntityResolver) -> Option<CandidateMemory> {
    let lower = sentence.to_lowercase();
    let triggers: &[&str] = &[
        "don't ",
        "do not ",
        "never ",
        "stop ",
        "avoid ",
        "must not ",
        "please don't ",
        "please do not ",
        "don't ever ",
        "do not ever ",
    ];

    for trigger in triggers {
        if let Some(pos) = lower.find(trigger) {
            let value_start = pos + trigger.len();
            let value = sentence[value_start..].trim();
            if value.is_empty() {
                continue;
            }
            let entity = resolver.resolve_subject("constraint");
            let mut meta = BTreeMap::new();
            meta.insert(
                "constraint_trigger".to_string(),
                (*trigger).trim().to_string(),
            );
            return Some(CandidateMemory {
                candidate_id: Uuid::new_v4().to_string(),
                observed_at: Utc::now(),
                entity,
                slot: Some("constraint".to_string()),
                value: Some(value.to_string()),
                raw_text: sentence.to_string(),
                source: Provenance {
                    source_type: SourceType::Chat,
                    source_id: String::new(),
                    source_label: None,
                    observed_by: None,
                    trust_weight: 1.0,
                },
                memory_type: MemoryType::Fact,
                confidence: 0.60,
                salience: 0.60,
                scope: Scope::Private,
                ttl: None,
                event_at: None,
                valid_from: None,
                valid_to: None,
                internal_layer: None,
                tags: vec!["constraint".to_string()],
                metadata: meta,
                is_retraction: true,
                thread_id: None,
                parent_memory_id: None,
            });
        }
    }
    None
}

/// Conditional qualifier patterns: "unless X", "except when X", "only if X",
/// "as long as X".
///
/// The condition clause is stored in `metadata["condition"]` so retrieval can
/// apply it as a filter override at query time.
fn try_conditional(sentence: &str, resolver: &EntityResolver) -> Option<CandidateMemory> {
    // Regex: capture the main clause before the trigger and the condition after.
    let re = Regex::new(r"(?i)^(.*?)\b(unless|except when|only if|as long as)\b(.+)$").ok()?;
    let caps = re.captures(sentence)?;
    let main_clause = caps.get(1)?.as_str().trim();
    let trigger = caps.get(2)?.as_str().trim();
    let condition = caps.get(3)?.as_str().trim();

    if condition.is_empty() {
        return None;
    }

    // The main clause becomes the value (what is conditionally true).
    let effective_value = if main_clause.is_empty() {
        sentence.to_string()
    } else {
        main_clause.to_string()
    };

    let entity = resolver.resolve_subject("conditional");
    let mut meta = BTreeMap::new();
    meta.insert("condition".to_string(), condition.to_string());
    meta.insert("condition_trigger".to_string(), trigger.to_string());

    Some(CandidateMemory {
        candidate_id: Uuid::new_v4().to_string(),
        observed_at: Utc::now(),
        entity,
        slot: Some("conditional".to_string()),
        value: Some(effective_value),
        raw_text: sentence.to_string(),
        source: Provenance {
            source_type: SourceType::Chat,
            source_id: String::new(),
            source_label: None,
            observed_by: None,
            trust_weight: 1.0,
        },
        memory_type: MemoryType::Fact,
        confidence: 0.50,
        salience: 0.45,
        scope: Scope::Private,
        ttl: None,
        event_at: None,
        valid_from: None,
        valid_to: None,
        internal_layer: None,
        tags: vec!["conditional".to_string()],
        metadata: meta,
        is_retraction: false,
        thread_id: None,
        parent_memory_id: None,
    })
}

/// Self-correction signals: "I changed my mind", "that was wrong", "actually",
/// "I meant", "correction:", "let me clarify".
///
/// Emits a candidate with `metadata["correction_hint"] = "true"` so downstream
/// belief updaters know to treat this as a retraction of the most recent memory
/// in the same context.
fn try_self_correction(sentence: &str, _resolver: &EntityResolver) -> Option<CandidateMemory> {
    let lower = sentence.to_lowercase();
    let triggers: &[&str] = &[
        "i changed my mind",
        "that was wrong",
        "i was wrong",
        "actually,",
        "actually ",
        "i meant ",
        "correction:",
        "let me clarify",
        "to clarify,",
        "to be clear,",
        "i made a mistake",
    ];

    let matched = triggers.iter().find(|&&t| lower.contains(t))?;

    let value = sentence.trim();
    let mut meta = BTreeMap::new();
    meta.insert("correction_hint".to_string(), "true".to_string());
    meta.insert(
        "correction_trigger".to_string(),
        (*matched).trim().to_string(),
    );

    Some(CandidateMemory {
        candidate_id: Uuid::new_v4().to_string(),
        observed_at: Utc::now(),
        entity: None,
        slot: Some("self_correction".to_string()),
        value: Some(value.to_string()),
        raw_text: sentence.to_string(),
        source: Provenance {
            source_type: SourceType::Chat,
            source_id: String::new(),
            source_label: None,
            observed_by: None,
            trust_weight: 1.0,
        },
        memory_type: MemoryType::Correction,
        confidence: 0.80,
        salience: 0.75,
        scope: Scope::Private,
        ttl: None,
        event_at: None,
        valid_from: None,
        valid_to: None,
        internal_layer: Some(MemoryLayer::Correction),
        tags: vec!["correction".to_string(), "self_correction".to_string()],
        metadata: meta,
        is_retraction: true,
        thread_id: None,
        parent_memory_id: None,
    })
}

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::entity_resolver::EntityResolver;
    use super::*;

    fn resolver() -> EntityResolver {
        EntityResolver::new(None)
    }

    #[test]
    fn instruction_from_now_on() {
        let results = ClaimExtractor::extract("From now on use metric units", &resolver());
        assert!(!results.is_empty());
        let c = &results[0];
        assert_eq!(c.internal_layer, Some(MemoryLayer::SelfModel));
        assert!(!c.is_retraction);
        assert!(c.metadata.contains_key("instruction_trigger"));
    }

    #[test]
    fn instruction_remember_to() {
        let results =
            ClaimExtractor::extract("Remember to add tests for all new code", &resolver());
        assert!(!results.is_empty());
        let c = &results[0];
        assert_eq!(c.internal_layer, Some(MemoryLayer::SelfModel));
    }

    #[test]
    fn negation_dont() {
        let results = ClaimExtractor::extract("Don't use tabs for indentation", &resolver());
        assert!(!results.is_empty());
        assert!(results[0].is_retraction);
        assert!(results[0].metadata.contains_key("constraint_trigger"));
    }

    #[test]
    fn negation_never() {
        let results = ClaimExtractor::extract("Never commit secrets to the repo", &resolver());
        assert!(!results.is_empty());
        assert!(results[0].is_retraction);
    }

    #[test]
    fn conditional_unless() {
        let results =
            ClaimExtractor::extract("Use verbose logging unless on production", &resolver());
        assert!(!results.is_empty());
        let c = &results[0];
        assert!(c.metadata.contains_key("condition"));
        assert_eq!(c.metadata["condition"], "on production");
    }

    #[test]
    fn conditional_except_when() {
        let results =
            ClaimExtractor::extract("Run full tests except when in CI draft mode", &resolver());
        assert!(!results.is_empty());
        let c = &results[0];
        assert!(c.metadata.contains_key("condition"));
    }

    #[test]
    fn self_correction_actually() {
        let results = ClaimExtractor::extract("Actually, I prefer spaces over tabs", &resolver());
        assert!(!results.is_empty());
        let c = &results[0];
        assert_eq!(c.memory_type, MemoryType::Correction);
        assert!(c.is_retraction);
        assert_eq!(c.metadata.get("correction_hint"), Some(&"true".to_string()));
    }

    #[test]
    fn self_correction_changed_mind() {
        let results =
            ClaimExtractor::extract("I changed my mind about the API design", &resolver());
        assert!(!results.is_empty());
        assert_eq!(results[0].memory_type, MemoryType::Correction);
    }

    #[test]
    fn is_are_still_works() {
        let results = ClaimExtractor::extract("The sky is blue", &resolver());
        assert!(!results.is_empty());
        assert_eq!(results[0].memory_type, MemoryType::Fact);
        assert!(!results[0].is_retraction);
    }
}
