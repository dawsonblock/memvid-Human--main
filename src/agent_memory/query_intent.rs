use super::enums::QueryIntent;

/// Rule-based retrieval intent detector.
#[derive(Debug, Default, Clone, Copy)]
pub struct QueryIntentDetector;

impl QueryIntentDetector {
    #[must_use]
    pub fn detect(&self, query: &str) -> QueryIntent {
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
