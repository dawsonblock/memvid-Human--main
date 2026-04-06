use chrono::{DateTime, Utc};

use super::enums::{MemoryLayer, MemoryType, QueryIntent};
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
            let memory_layer = hit.memory_layer.or_else(|| hit.memory_type.map(MemoryType::memory_layer));
            if let Some(layer) = memory_layer {
                score += Self::type_bonus(intent, layer);
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

    fn type_bonus(intent: QueryIntent, memory_layer: MemoryLayer) -> f32 {
        match (intent, memory_layer) {
            (QueryIntent::PreferenceLookup, MemoryLayer::SelfModel) => 1.4,
            (QueryIntent::TaskState, MemoryLayer::GoalState) => 1.5,
            (QueryIntent::TaskState, MemoryLayer::Episode) => 0.9,
            (QueryIntent::TaskState, MemoryLayer::Procedure) => 0.55,
            (QueryIntent::CurrentFact, MemoryLayer::Belief) => 1.1,
            (QueryIntent::HistoricalFact, MemoryLayer::Episode) => 0.9,
            (QueryIntent::HistoricalFact, MemoryLayer::Belief) => 0.8,
            (QueryIntent::EpisodicRecall, MemoryLayer::Episode) => 1.3,
            (QueryIntent::SemanticBackground, MemoryLayer::Procedure) => 0.25,
            (_, MemoryLayer::Trace) => -0.2,
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
