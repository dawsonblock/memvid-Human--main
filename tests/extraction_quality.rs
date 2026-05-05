//! Integration tests for the extraction pipeline quality.
//!
//! Validates that each sub-extractor (preferences, claims, scorer, deduplication)
//! and the end-to-end pipeline produce correct output for known inputs.

mod common;

use memvid_core::agent_memory::enums::MemoryType;
use memvid_core::agent_memory::extraction::{
    RawInputProcessor, candidate_scorer::CandidateScorer, claim_extractor::ClaimExtractor,
    entity_resolver::EntityResolver, preference_extractor::PreferenceExtractor,
    provider::MergedExtractionValidator,
};
use memvid_core::agent_memory::schemas::IngestContext;

// ── Preference extractor ─────────────────────────────────────────────────────

#[test]
fn pref_i_prefer_slot_is_prefers() {
    let resolver = EntityResolver::new(Some("user".to_string()));
    let candidates = PreferenceExtractor::extract("I prefer dark mode in my editor.", &resolver);
    assert!(
        !candidates.is_empty(),
        "should find at least one preference"
    );
    let c = candidates
        .iter()
        .find(|c| c.slot.as_deref() == Some("prefers"))
        .expect("slot 'prefers' not found");
    assert_eq!(c.entity, Some("user".to_string()));
    assert!(
        c.value.as_deref().unwrap_or("").contains("dark mode"),
        "value should contain 'dark mode', got: {:?}",
        c.value
    );
}

#[test]
fn pref_i_like_slot_is_likes() {
    let resolver = EntityResolver::new(Some("user".to_string()));
    let candidates =
        PreferenceExtractor::extract("I like Rust for systems programming.", &resolver);
    assert!(!candidates.is_empty());
    let c = candidates
        .iter()
        .find(|c| c.slot.as_deref() == Some("likes"))
        .expect("slot 'likes' not found");
    assert_eq!(c.entity, Some("user".to_string()));
    let val = c.value.as_deref().unwrap_or("").to_lowercase();
    assert!(
        val.contains("rust"),
        "value should contain 'rust', got: {:?}",
        c.value
    );
}

#[test]
fn pref_i_hate_slot_is_dislikes() {
    let resolver = EntityResolver::new(Some("user".to_string()));
    let candidates = PreferenceExtractor::extract("I hate boilerplate code.", &resolver);
    assert!(!candidates.is_empty());
    let c = candidates
        .iter()
        .find(|c| c.slot.as_deref() == Some("dislikes"))
        .expect("slot 'dislikes' not found");
    assert!(
        c.value.as_deref().unwrap_or("").contains("boilerplate"),
        "value should contain 'boilerplate', got: {:?}",
        c.value
    );
}

#[test]
fn pref_confidence_and_salience_values() {
    let resolver = EntityResolver::new(Some("user".to_string()));
    let candidates = PreferenceExtractor::extract("I prefer Vim.", &resolver);
    assert!(!candidates.is_empty());
    let c = &candidates[0];
    assert!(
        (c.confidence - 0.75).abs() < 0.01,
        "confidence should be ~0.75, got {}",
        c.confidence
    );
    assert!(
        (c.salience - 0.70).abs() < 0.01,
        "salience should be ~0.70, got {}",
        c.salience
    );
}

#[test]
fn pref_no_entity_hint_entity_is_none() {
    // When entity_hint is None, "I" pronoun resolves to None.
    let resolver = EntityResolver::new(None);
    let candidates = PreferenceExtractor::extract("I prefer tabs over spaces.", &resolver);
    assert!(!candidates.is_empty());
    let c = candidates
        .iter()
        .find(|c| c.slot.as_deref() == Some("prefers"))
        .expect("slot 'prefers' not found");
    assert_eq!(c.entity, None, "entity should be None when hint is absent");
}

// ── Claim extractor ──────────────────────────────────────────────────────────

#[test]
fn claim_is_are_verb_becomes_slot() {
    let resolver = EntityResolver::new(None);
    let candidates = ClaimExtractor::extract("Alice is a software engineer.", &resolver);
    assert!(!candidates.is_empty(), "should extract at least one claim");
    let c = candidates
        .iter()
        .find(|c| c.slot.as_deref() == Some("is"))
        .expect("slot 'is' not found");
    assert_eq!(c.entity, Some("Alice".to_string()));
    assert!(
        c.value
            .as_deref()
            .unwrap_or("")
            .contains("software engineer"),
        "value should contain 'software engineer', got: {:?}",
        c.value
    );
}

#[test]
fn claim_has_slot() {
    let resolver = EntityResolver::new(None);
    let candidates = ClaimExtractor::extract("Bob has a computer.", &resolver);
    assert!(!candidates.is_empty());
    let c = candidates
        .iter()
        .find(|c| c.slot.as_deref() == Some("has"))
        .expect("slot 'has' not found");
    assert_eq!(c.entity, Some("Bob".to_string()));
    assert!(
        c.value.as_deref().unwrap_or("").contains("computer"),
        "value should contain 'computer', got: {:?}",
        c.value
    );
}

#[test]
fn claim_colon_slot_is_literal_value() {
    let resolver = EntityResolver::new(None);
    // try_colon matches "label: value" — slot is always "value".
    let candidates = ClaimExtractor::extract("language: Rust", &resolver);
    assert!(!candidates.is_empty(), "should extract colon claim");
    let c = candidates
        .iter()
        .find(|c| c.slot.as_deref() == Some("value"))
        .expect("slot 'value' not found for colon pattern");
    assert_eq!(c.entity, Some("language".to_string()));
    assert_eq!(c.value.as_deref(), Some("Rust"));
}

#[test]
fn claim_instruction_trigger_slot_and_confidence() {
    let resolver = EntityResolver::new(None);
    let candidates = ClaimExtractor::extract("From now on use tabs for indentation.", &resolver);
    assert!(!candidates.is_empty(), "should extract instruction");
    let c = candidates
        .iter()
        .find(|c| c.slot.as_deref() == Some("instruction"))
        .expect("slot 'instruction' not found");
    // Instruction confidence is 0.70 per spec.
    assert!(
        (c.confidence - 0.70).abs() < 0.01,
        "instruction confidence should be ~0.70, got {}",
        c.confidence
    );
    assert!(!c.is_retraction, "instructions are not retractions");
}

#[test]
fn claim_negation_is_retraction() {
    let resolver = EntityResolver::new(None);
    let candidates = ClaimExtractor::extract("Don't use Comic Sans.", &resolver);
    assert!(!candidates.is_empty(), "should extract negation constraint");
    let c = candidates
        .iter()
        .find(|c| c.slot.as_deref() == Some("constraint"))
        .expect("slot 'constraint' not found");
    assert!(
        c.is_retraction,
        "negation constraint must be marked is_retraction=true"
    );
    assert!(
        c.value.as_deref().unwrap_or("").contains("Comic Sans")
            || c.value.as_deref().unwrap_or("").contains("Comic"),
        "value should reference 'Comic Sans', got: {:?}",
        c.value
    );
}

#[test]
fn claim_empty_text_yields_empty() {
    let resolver = EntityResolver::new(None);
    let candidates = ClaimExtractor::extract("", &resolver);
    assert!(
        candidates.is_empty(),
        "claim extractor on empty input should return empty vec"
    );
}

#[test]
fn claim_multiple_sentences_multiple_candidates() {
    let resolver = EntityResolver::new(None);
    let text = "Alice is a manager. Bob has a dog.";
    let candidates = ClaimExtractor::extract(text, &resolver);
    assert!(
        candidates.len() >= 2,
        "should extract at least 2 candidates from 2 sentences, got {}",
        candidates.len()
    );
}

#[test]
fn claim_memory_type_is_fact() {
    let resolver = EntityResolver::new(None);
    let candidates = ClaimExtractor::extract("Carol is a designer.", &resolver);
    assert!(!candidates.is_empty());
    for c in &candidates {
        assert_eq!(
            c.memory_type,
            MemoryType::Fact,
            "claim extractor produces Fact type"
        );
    }
}

// ── CandidateScorer ──────────────────────────────────────────────────────────

#[test]
fn scorer_full_svo_raises_confidence_to_floor() {
    // Full SVO (entity + slot + value all present) → confidence must be ≥ 0.7.
    let mut c = common::candidate("alice", "is", "a developer", "Alice is a developer");
    c.memory_type = MemoryType::Fact;
    c.confidence = 0.1; // deliberately low
    c.salience = 0.05;
    let scorer = CandidateScorer::default();
    scorer.score(&mut c);
    assert!(
        c.confidence >= 0.7,
        "full SVO should lift confidence to ≥ 0.7, got {}",
        c.confidence
    );
}

#[test]
fn scorer_partial_svo_raises_confidence_to_partial_floor() {
    // Partial candidate (entity + slot, no value) → confidence must be ≥ 0.4.
    let mut c = common::candidate("alice", "prefers", "", "Alice prefers dark mode");
    c.memory_type = MemoryType::Fact;
    c.confidence = 0.1;
    c.salience = 0.05;
    let scorer = CandidateScorer::default();
    scorer.score(&mut c);
    assert!(
        c.confidence >= 0.4,
        "partial SVO should lift confidence to ≥ 0.4, got {}",
        c.confidence
    );
}

#[test]
fn scorer_skill_type_floors_both_confidence_and_salience() {
    // MemoryType::Skill → both confidence ≥ 0.65 AND salience ≥ 0.65.
    let mut c = common::candidate("alice", "codes", "rust", "Alice codes Rust");
    c.memory_type = MemoryType::Skill;
    c.confidence = 0.1;
    c.salience = 0.05;
    let scorer = CandidateScorer::default();
    scorer.score(&mut c);
    assert!(
        c.confidence >= 0.65,
        "Skill confidence should be ≥ 0.65, got {}",
        c.confidence
    );
    assert!(
        c.salience >= 0.65,
        "Skill salience should be ≥ 0.65, got {}",
        c.salience
    );
}

#[test]
fn scorer_salience_floor_is_half_confidence() {
    // Salience floor = confidence * 0.5; verify that salience is raised
    // when it falls below that floor.
    let mut c = common::candidate("alice", "is", "a tester", "Alice is a tester");
    c.memory_type = MemoryType::Fact;
    c.confidence = 0.8;
    c.salience = 0.0; // below floor of 0.4
    let scorer = CandidateScorer::default();
    scorer.score(&mut c);
    assert!(
        c.salience >= c.confidence * 0.5 - 0.01,
        "salience should be at least confidence * 0.5, confidence={} salience={}",
        c.confidence,
        c.salience
    );
}

// ── Deduplication (MergedExtractionValidator) ────────────────────────────────

#[test]
fn dedup_keeps_highest_confidence_candidate() {
    let validator = MergedExtractionValidator;
    let mut low = common::candidate("alice", "prefers", "dark mode", "");
    low.confidence = 0.4;
    let mut high = common::candidate("alice", "prefers", "dark mode", "");
    high.confidence = 0.9;
    let result = validator.deduplicate(vec![low, high]);
    assert_eq!(result.len(), 1, "duplicates should be collapsed to one");
    assert!(
        result[0].confidence >= 0.9,
        "surviving candidate should have highest confidence, got {}",
        result[0].confidence
    );
}

#[test]
fn dedup_generic_traces_are_always_kept() {
    // Generic trace: entity=None, slot=None, value=None — never deduplicated.
    let validator = MergedExtractionValidator;
    let trace1 = common::candidate("", "", "", "first trace");
    let trace2 = common::candidate("", "", "", "second trace");
    let result = validator.deduplicate(vec![trace1, trace2]);
    assert_eq!(
        result.len(),
        2,
        "generic traces (all-None fields) should never be deduplicated"
    );
}

#[test]
fn dedup_key_is_case_insensitive() {
    // "Alice/prefers/dark mode" and "alice/prefers/dark mode" are the same key.
    let validator = MergedExtractionValidator;
    let mut upper = common::candidate("Alice", "prefers", "dark mode", "");
    upper.confidence = 0.6;
    let mut lower = common::candidate("alice", "prefers", "dark mode", "");
    lower.confidence = 0.8;
    let result = validator.deduplicate(vec![upper, lower]);
    assert_eq!(
        result.len(),
        1,
        "case-insensitive key should collapse both entries into one"
    );
}

// ── End-to-end pipeline ──────────────────────────────────────────────────────

#[test]
fn pipeline_empty_input_returns_single_generic_trace() {
    let pipeline = RawInputProcessor::new();
    let ctx = IngestContext::default();
    let results = pipeline.process("", &ctx);
    assert_eq!(
        results.len(),
        1,
        "empty input should produce exactly one generic trace"
    );
    let trace = &results[0];
    assert!(
        trace.entity.is_none() && trace.slot.is_none(),
        "generic trace should have all-None structured fields"
    );
}

#[test]
fn pipeline_preference_text_yields_candidates() {
    let pipeline = RawInputProcessor::new();
    let ctx = IngestContext::default();
    let results = pipeline.process("I prefer dark mode in my editor.", &ctx);
    assert!(
        !results.is_empty(),
        "preference sentence should produce at least one candidate"
    );
    assert!(
        results.iter().any(|c| c.slot.as_deref() == Some("prefers")),
        "should find a 'prefers' slot candidate"
    );
}

#[test]
fn pipeline_context_scope_stamped_on_all_candidates() {
    use memvid_core::agent_memory::enums::Scope;
    let pipeline = RawInputProcessor::new();
    let mut ctx = IngestContext::default();
    ctx.scope = Scope::Task;
    let results = pipeline.process("I prefer dark mode.", &ctx);
    for c in &results {
        assert_eq!(
            c.scope,
            Scope::Task,
            "every candidate should inherit context scope"
        );
    }
}

#[test]
fn pipeline_deduplicates_overlapping_extractions() {
    // Process the same text twice and check that dedup actually fires.
    // Two identical claims in one string should yield only one structured output.
    let pipeline = RawInputProcessor::new();
    let ctx = IngestContext::default();
    // Two identical sentences separated by a period — both produce same key.
    let results = pipeline.process("Alice is a developer. Alice is a developer.", &ctx);
    let structured: Vec<_> = results.iter().filter(|c| c.entity.is_some()).collect();
    assert!(
        structured.len() <= 1,
        "duplicate SVO sentences should be collapsed, got {} structured results",
        structured.len()
    );
}

// ── LLM mock backends (requires test_helpers feature) ───────────────────────

#[cfg(feature = "test_helpers")]
mod llm_mock_tests {
    use memvid_core::agent_memory::extraction::{
        LLMStructuredExtractor, MemoryExtractionProvider, MockLLMExtractionBackend,
        MockLLMExtractionBackendKeyword,
    };
    use memvid_core::agent_memory::schemas::IngestContext;

    #[test]
    fn mock_llm_fixed_response_produces_candidates() {
        let json = r#"[{"entity":"alice","slot":"role","value":"engineer","memory_type":"fact","confidence":0.9,"salience":0.8}]"#;
        let extractor =
            LLMStructuredExtractor::with_default_prompt(Box::new(MockLLMExtractionBackend {
                response: json.to_string(),
            }));
        let results = extractor.extract("Alice is an engineer", &IngestContext::default());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entity.as_deref(), Some("alice"));
        assert_eq!(results[0].slot.as_deref(), Some("role"));
        assert_eq!(results[0].value.as_deref(), Some("engineer"));
    }

    #[test]
    fn mock_llm_keyword_routing_picks_matching_entry() {
        let alice_json = r#"[{"entity":"alice","slot":"skill","value":"rust","memory_type":"fact","confidence":0.85,"salience":0.75}]"#;
        let bob_json = r#"[{"entity":"bob","slot":"skill","value":"python","memory_type":"fact","confidence":0.85,"salience":0.75}]"#;
        let extractor = LLMStructuredExtractor::with_default_prompt(Box::new(
            MockLLMExtractionBackendKeyword {
                entries: vec![
                    ("alice".to_string(), alice_json.to_string()),
                    ("bob".to_string(), bob_json.to_string()),
                ],
            },
        ));
        // Prompt contains "alice" → should route to alice_json.
        let results = extractor.extract("Tell me about Alice", &IngestContext::default());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entity.as_deref(), Some("alice"));
    }
}
