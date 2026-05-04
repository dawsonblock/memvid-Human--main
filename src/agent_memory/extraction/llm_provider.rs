//! LLM-backed structured extraction provider.
//!
//! This module defines a single-method [`LLMExtractionBackend`] trait,
//! a [`LLMStructuredExtractor`] that implements [`MemoryExtractionProvider`]
//! (and thus plugs into the standard extraction pipeline), and — behind the
//! `api_embed` feature — a concrete [`OpenAIExtractionBackend`] that calls a
//! Chat Completions-compatible endpoint via a blocking HTTP client.
//!
//! Architecture
//! ─────────────
//! ```text
//! LLMStructuredExtractor
//!   ├─ backend: Box<dyn LLMExtractionBackend>   ← pluggable
//!   └─ system_prompt: String                     ← customisable
//!
//! LLMExtractionBackend::call(prompt) → JSON string
//!   └─ parse → Vec<RawExtractionItem>
//!       └─ validate + build CandidateMemory
//! ```
//!
//! The `api_embed` feature brings in `reqwest` (blocking) so that
//! [`OpenAIExtractionBackend`] can be compiled.  Without the feature the
//! trait and the struct still compile — you just need to supply your own
//! [`LLMExtractionBackend`].

use std::collections::BTreeMap;

use serde::Deserialize;
use uuid::Uuid;

use super::super::enums::MemoryType;
use super::super::errors::Result;
use super::super::schemas::{CandidateMemory, IngestContext, Provenance};
use super::provider::MemoryExtractionProvider;

// ── Public system-prompt helper ───────────────────────────────────────────────

/// Return the default JSON-extraction system prompt.
///
/// The prompt instructs an LLM to emit a JSON array whose elements match
/// [`RawExtractionItem`] so that [`LLMStructuredExtractor`] can parse them
/// directly.
#[must_use]
pub fn default_system_prompt() -> &'static str {
    r#"You are a structured memory extraction assistant.
Given a passage of text and optional context hints you must extract all
factual claims, preferences, goals, skills, or constraints that are worth
storing as long-term memories.

Respond with a JSON array (and nothing else — no markdown, no explanation).
Each element must have these fields:
  entity      : string or null — subject the fact is about
  slot        : string or null — typed relationship or attribute name
  value       : string         — factual value (required, non-empty)
  memory_type : one of "fact", "preference", "goal_state", "skill",
                "instruction", "constraint", "decision", "episode", "trace"
  confidence  : float in [0.0, 1.0]
  salience    : float in [0.0, 1.0] — how important / reusable this memory is

Rules:
- Omit trivial, ambiguous, or duplicate items.
- Never fabricate information not present in the text.
- Return an empty array ([]) if nothing is worth extracting.
"#
}

// ── Backend trait ─────────────────────────────────────────────────────────────

/// Single-method backend: send `prompt` (including system instructions and
/// user text), receive a raw string response (expected to be JSON).
///
/// Implementations are responsible for auth, retries, and error formatting.
/// No HTTP client is mandated — anything that can return a `String` works.
pub trait LLMExtractionBackend: Send + Sync {
    /// Send `prompt` to the LLM and return its raw text response.
    fn call(&self, prompt: &str) -> Result<String>;
}

// ── Raw deserialization target ────────────────────────────────────────────────

/// Intermediate struct mirroring the JSON schema the LLM is asked to emit.
#[derive(Debug, Deserialize)]
struct RawExtractionItem {
    entity: Option<String>,
    slot: Option<String>,
    value: String,
    memory_type: String,
    confidence: f32,
    salience: f32,
}

// ── LLMStructuredExtractor ────────────────────────────────────────────────────

/// [`MemoryExtractionProvider`] backed by an [`LLMExtractionBackend`].
///
/// # Usage
///
/// ```rust,ignore
/// let extractor = LLMStructuredExtractor::new(
///     Box::new(my_backend),
///     default_system_prompt().to_string(),
/// );
/// // Register with extraction pipeline:
/// pipeline.register_provider(Box::new(extractor));
/// ```
pub struct LLMStructuredExtractor {
    backend: Box<dyn LLMExtractionBackend>,
    system_prompt: String,
}

impl LLMStructuredExtractor {
    /// Create a new extractor with the given backend and system prompt.
    #[must_use]
    pub fn new(backend: Box<dyn LLMExtractionBackend>, system_prompt: String) -> Self {
        Self {
            backend,
            system_prompt,
        }
    }

    /// Create a new extractor using [`default_system_prompt`].
    #[must_use]
    pub fn with_default_prompt(backend: Box<dyn LLMExtractionBackend>) -> Self {
        Self::new(backend, default_system_prompt().to_string())
    }

    // Build the full prompt string from system instructions + user text + hints.
    fn build_prompt(&self, text: &str, context: &IngestContext) -> String {
        let mut lines = vec![self.system_prompt.as_str().to_string()];

        if let Some(entity) = context.entity_hint.as_deref() {
            if !entity.is_empty() {
                lines.push(format!("Entity hint: {entity}"));
            }
        }
        if !context.tags.is_empty() {
            lines.push(format!("Tags: {}", context.tags.join(", ")));
        }

        lines.push(String::new());
        lines.push("Text to extract from:".to_string());
        lines.push(text.to_string());
        lines.join("\n")
    }

    // Parse a JSON string → vec of validated CandidateMemory.
    fn parse_response(
        &self,
        json: &str,
        context: &IngestContext,
        raw_text: &str,
    ) -> Vec<CandidateMemory> {
        // Strip optional markdown code fences before parsing.
        let json = json.trim();
        let json = json
            .strip_prefix("```json")
            .or_else(|| json.strip_prefix("```"))
            .map(|s| s.trim_end_matches("```").trim())
            .unwrap_or(json);

        let items: Vec<RawExtractionItem> = match serde_json::from_str(json) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let provenance = Provenance {
            source_type: context.source_type,
            source_id: "llm_extractor".to_string(),
            source_label: Some("LLMStructuredExtractor".to_string()),
            observed_by: None,
            trust_weight: 0.7,
        };

        items
            .into_iter()
            .filter_map(|item| self.validate_item(item, context, raw_text, &provenance))
            .collect()
    }

    fn validate_item(
        &self,
        item: RawExtractionItem,
        context: &IngestContext,
        raw_text: &str,
        provenance: &Provenance,
    ) -> Option<CandidateMemory> {
        // Hard validation: value must be non-empty.
        let value = item.value.trim().to_string();
        if value.is_empty() {
            return None;
        }

        // Clamp numeric fields.
        let confidence = item.confidence.clamp(0.0, 1.0);
        let salience = item.salience.clamp(0.0, 1.0);

        // Parse memory_type; discard if unrecognisable.
        let memory_type = parse_memory_type(&item.memory_type)?;

        let entity = item
            .entity
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let slot = item
            .slot
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        Some(CandidateMemory {
            candidate_id: Uuid::new_v4().to_string(),
            observed_at: chrono::Utc::now(),
            entity,
            slot,
            value: Some(value),
            raw_text: raw_text.to_string(),
            source: provenance.clone(),
            memory_type,
            confidence,
            salience,
            scope: context.scope,
            ttl: None,
            event_at: None,
            valid_from: None,
            valid_to: None,
            internal_layer: None,
            tags: context.tags.clone(),
            metadata: BTreeMap::new(),
            is_retraction: false,
            thread_id: None,
            parent_memory_id: None,
        })
    }
}

impl MemoryExtractionProvider for LLMStructuredExtractor {
    fn extract(&self, text: &str, context: &IngestContext) -> Vec<CandidateMemory> {
        let prompt = self.build_prompt(text, context);
        match self.backend.call(&prompt) {
            Ok(json) => self.parse_response(&json, context, text),
            Err(_) => Vec::new(), // provider must never panic
        }
    }

    fn name(&self) -> &'static str {
        "LLMStructuredExtractor"
    }
}

// ── Parse helper ──────────────────────────────────────────────────────────────

fn parse_memory_type(s: &str) -> Option<MemoryType> {
    match s.trim().to_lowercase().as_str() {
        "fact" => Some(MemoryType::Fact),
        "preference" => Some(MemoryType::Preference),
        "goal_state" | "goal" => Some(MemoryType::GoalState),
        "skill" => Some(MemoryType::Skill),
        "instruction" => Some(MemoryType::Instruction),
        "constraint" => Some(MemoryType::Constraint),
        "decision" => Some(MemoryType::Decision),
        "episode" => Some(MemoryType::Episode),
        "trace" => Some(MemoryType::Trace),
        "correction" => Some(MemoryType::Correction),
        _ => None,
    }
}

// ── OpenAIExtractionBackend (api_embed feature) ───────────────────────────────

/// Blocking HTTP backend that calls an OpenAI Chat Completions-compatible
/// endpoint.
///
/// The `api_embed` feature must be enabled to compile this struct.
/// Credentials are supplied at construction time — never read from env here
/// (callers decide the injection strategy).
///
/// # Security
///
/// `api_key` is stored in plain memory.  Callers are responsible for
/// ensuring the key is obtained from a secure store (e.g. env var, vault)
/// and not logged.
#[cfg(feature = "api_embed")]
pub struct OpenAIExtractionBackend {
    endpoint: String,
    api_key: String,
    model: String,
    client: reqwest::blocking::Client,
}

#[cfg(feature = "api_embed")]
impl OpenAIExtractionBackend {
    /// Create a backend targeting `endpoint` (e.g.
    /// `"https://api.openai.com/v1/chat/completions"`) with the given
    /// `api_key` and `model` (e.g. `"gpt-4o-mini"`).
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be constructed.
    pub fn new(endpoint: impl Into<String>, api_key: impl Into<String>, model: impl Into<String>) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| AgentMemoryError::Store { reason: e.to_string() })?;
        Ok(Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            model: model.into(),
            client,
        })
    }
}

#[cfg(feature = "api_embed")]
impl LLMExtractionBackend for OpenAIExtractionBackend {
    fn call(&self, prompt: &str) -> Result<String> {
        use serde_json::json;

        let body = json!({
            "model": self.model,
            "messages": [
                { "role": "user", "content": prompt }
            ],
            "temperature": 0.0
        });

        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .map_err(|e| AgentMemoryError::Store { reason: e.to_string() })?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(AgentMemoryError::Store {
                reason: format!("LLM endpoint returned HTTP {status}"),
            });
        }

        let json: serde_json::Value = response
            .json()
            .map_err(|e| AgentMemoryError::Store { reason: e.to_string() })?;

        // Extract content from choices[0].message.content
        let content = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentMemoryError::Store {
                reason: "unexpected LLM response shape".to_string(),
            })?;

        Ok(content.to_string())
    }
}

// ── Scope/SourceType used from enums for clarity in tests ────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::super::enums::{Scope, SourceType};
    use super::super::super::errors::AgentMemoryError;

    struct EchoBackend(String);

    impl LLMExtractionBackend for EchoBackend {
        fn call(&self, _prompt: &str) -> Result<String> {
            Ok(self.0.clone())
        }
    }

    fn ctx() -> IngestContext {
        IngestContext {
            source_type: SourceType::Chat,
            scope: Scope::Private,
            entity_hint: Some("Alice".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn extracts_valid_item() {
        let json = r#"[{"entity":"Alice","slot":"role","value":"engineer","memory_type":"fact","confidence":0.9,"salience":0.8}]"#;
        let extractor = LLMStructuredExtractor::with_default_prompt(Box::new(EchoBackend(json.to_string())));
        let results = extractor.extract("Alice is a software engineer.", &ctx());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entity.as_deref(), Some("Alice"));
        assert_eq!(results[0].value.as_deref(), Some("engineer"));
    }

    #[test]
    fn discards_empty_value() {
        let json = r#"[{"entity":"Bob","slot":"role","value":"","memory_type":"fact","confidence":0.9,"salience":0.8}]"#;
        let extractor = LLMStructuredExtractor::with_default_prompt(Box::new(EchoBackend(json.to_string())));
        let results = extractor.extract("text", &ctx());
        assert!(results.is_empty(), "empty value should be discarded");
    }

    #[test]
    fn discards_unknown_memory_type() {
        let json = r#"[{"entity":"Carol","slot":"mood","value":"happy","memory_type":"unknown_type","confidence":0.9,"salience":0.7}]"#;
        let extractor = LLMStructuredExtractor::with_default_prompt(Box::new(EchoBackend(json.to_string())));
        let results = extractor.extract("text", &ctx());
        assert!(results.is_empty(), "unknown memory_type should be discarded");
    }

    #[test]
    fn tolerates_backend_error() {
        struct FailBackend;
        impl LLMExtractionBackend for FailBackend {
            fn call(&self, _: &str) -> Result<String> {
                Err(AgentMemoryError::Store { reason: "oops".to_owned() })
            }
        }
        let extractor = LLMStructuredExtractor::with_default_prompt(Box::new(FailBackend));
        let results = extractor.extract("text", &ctx());
        assert!(results.is_empty(), "error should produce empty vec, not panic");
    }

    #[test]
    fn strips_markdown_fences() {
        let json = "```json\n[{\"entity\":null,\"slot\":null,\"value\":\"remember this\",\"memory_type\":\"trace\",\"confidence\":0.5,\"salience\":0.4}]\n```";
        let extractor = LLMStructuredExtractor::with_default_prompt(Box::new(EchoBackend(json.to_string())));
        let results = extractor.extract("text", &ctx());
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn clamps_out_of_range_scores() {
        let json = r#"[{"entity":null,"slot":null,"value":"test","memory_type":"fact","confidence":1.5,"salience":-0.3}]"#;
        let extractor = LLMStructuredExtractor::with_default_prompt(Box::new(EchoBackend(json.to_string())));
        let results = extractor.extract("text", &ctx());
        assert_eq!(results.len(), 1);
        assert!(results[0].confidence <= 1.0);
        assert!(results[0].salience >= 0.0);
    }
}
