//! Convenience text-to-intent helpers for obvious retrieval cases.
//!
//! Typed `RetrievalQuery` construction remains the authoritative path when a caller needs exact
//! retrieval semantics. The helpers in this module are intentionally small rule-based shorthands.

use super::enums::QueryIntent;
use super::schemas::RetrievalQuery;

/// Rule-based retrieval intent detector.
#[derive(Debug, Default, Clone, Copy)]
pub struct QueryIntentDetector;

impl QueryIntentDetector {
    #[must_use]
    pub fn detect(query: &str) -> QueryIntent {
        let lower = query.to_lowercase();
        if ["prefer", "favorite", "like", "dislike", "setting"]
            .iter()
            .any(|term| lower.contains(term))
        {
            QueryIntent::PreferenceLookup
        } else if ["goal", "task", "todo", "status", "working on", "blocked"]
            .iter()
            .any(|term| lower.contains(term))
        {
            QueryIntent::TaskState
        } else if ["remember when", "what happened", "last time", "episode"]
            .iter()
            .any(|term| lower.contains(term))
        {
            QueryIntent::EpisodicRecall
        } else if ["as of", "used to", "before", "previously", "historical"]
            .iter()
            .any(|term| lower.contains(term))
        {
            QueryIntent::HistoricalFact
        } else if lower.starts_with("what is")
            || lower.starts_with("where is")
            || lower.starts_with("who is")
            || lower.contains("current")
        {
            QueryIntent::CurrentFact
        } else {
            QueryIntent::SemanticBackground
        }
    }
}

impl RetrievalQuery {
    /// Convenience constructor for obvious text-only queries.
    ///
    /// This is a small rule-based wrapper around `QueryIntentDetector`; callers that care about
    /// exact query semantics should build `RetrievalQuery` explicitly.
    #[must_use]
    pub fn from_text(query_text: impl Into<String>) -> Self {
        let query_text = query_text.into();
        Self {
            intent: QueryIntentDetector::detect(&query_text),
            query_text,
            entity: None,
            slot: None,
            scope: None,
            top_k: 5,
            as_of: None,
            include_expired: false,
        }
    }
}
