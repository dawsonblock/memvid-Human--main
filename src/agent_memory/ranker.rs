use chrono::{DateTime, Utc};

use super::enums::{MemoryType, QueryIntent};
use super::schemas::RetrievalHit;

/// Deterministic reranker for governed retrieval hits.
#[derive(Debug, Default, Clone, Copy)]
pub struct Ranker;

impl Ranker {
    #[must_use]
    pub fn rerank(
        &self,
        mut hits: Vec<RetrievalHit>,
        intent: QueryIntent,
        now: DateTime<Utc>,
    ) -> Vec<RetrievalHit> {
        for hit in &mut hits {
            let mut score = hit.score;
            if let Some(memory_type) = hit.memory_type {
                score += Self::type_bonus(intent, memory_type);
                score += Self::recency_bonus(intent, hit.timestamp, now);
            }
            if hit.from_belief {
                score += 1.2;
            }
            if hit.expired {
                score -= 1.0;
            }
            hit.score = score;
        }
        hits.sort_by(|left, right| right.score.total_cmp(&left.score));
        hits
    }

    fn type_bonus(intent: QueryIntent, memory_type: MemoryType) -> f32 {
        match (intent, memory_type) {
            (QueryIntent::PreferenceLookup, MemoryType::Preference) => 1.4,
            (QueryIntent::TaskState, MemoryType::GoalState) => 1.5,
            (QueryIntent::TaskState, MemoryType::Episode) => 0.9,
            (QueryIntent::CurrentFact, MemoryType::Fact) => 1.1,
            (QueryIntent::HistoricalFact, MemoryType::Episode) => 0.6,
            (QueryIntent::HistoricalFact, MemoryType::Fact) => 0.8,
            (QueryIntent::EpisodicRecall, MemoryType::Episode) => 1.3,
            (_, MemoryType::Trace) => -0.2,
            _ => 0.0,
        }
    }

    fn recency_bonus(intent: QueryIntent, timestamp: DateTime<Utc>, now: DateTime<Utc>) -> f32 {
        let age_days = (now.timestamp() - timestamp.timestamp()).max(0) as f32 / 86_400.0;
        match intent {
            QueryIntent::TaskState | QueryIntent::EpisodicRecall => {
                (1.0 - (age_days / 30.0)).clamp(0.0, 1.0)
            }
            QueryIntent::HistoricalFact => 0.0,
            _ => (0.5 - (age_days / 180.0)).clamp(0.0, 0.5),
        }
    }
}
