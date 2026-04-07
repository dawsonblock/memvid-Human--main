use chrono::{DateTime, Utc};

use super::enums::{MemoryLayer, MemoryType, ProcedureStatus, QueryIntent};
use super::schemas::RetrievalHit;

const RETRIEVAL_ROLE_KEY: &str = "retrieval_role";

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
            let memory_layer = hit
                .memory_layer
                .or_else(|| hit.memory_type.map(MemoryType::memory_layer));
            if let Some(layer) = memory_layer {
                score += Self::type_bonus(intent, hit, layer);
                score += Self::recency_bonus(hit, layer, now);
            }
            score += Self::role_bonus(hit);
            if hit.expired {
                score -= 1.0;
            }
            score += Self::belief_bonus(intent, hit);
            score += Self::procedure_lifecycle_penalty(hit);
            hit.score = score;
        }
        hits.sort_by(|left, right| right.score.total_cmp(&left.score));
        hits
    }

    fn type_bonus(intent: QueryIntent, hit: &RetrievalHit, memory_layer: MemoryLayer) -> f32 {
        match (intent, memory_layer) {
            (QueryIntent::CurrentFact, MemoryLayer::Belief) => 2.5,
            (QueryIntent::CurrentFact, MemoryLayer::Episode) => 0.45,
            (QueryIntent::CurrentFact, MemoryLayer::Trace) => -0.8,
            (QueryIntent::HistoricalFact, MemoryLayer::Episode) => 2.0,
            (QueryIntent::HistoricalFact, MemoryLayer::Belief) => 1.1,
            (QueryIntent::HistoricalFact, MemoryLayer::Trace) => -0.6,
            (QueryIntent::PreferenceLookup, MemoryLayer::SelfModel) => 2.2,
            (QueryIntent::PreferenceLookup, MemoryLayer::Episode) => 0.6,
            (QueryIntent::PreferenceLookup, MemoryLayer::Trace) => -0.6,
            (QueryIntent::TaskState, MemoryLayer::GoalState) => {
                2.4 + Self::goal_state_priority_bonus(hit)
            }
            (QueryIntent::TaskState, MemoryLayer::Episode) => 1.05,
            (QueryIntent::TaskState, MemoryLayer::Procedure) => 0.9,
            (QueryIntent::TaskState, MemoryLayer::Trace) => -0.7,
            (QueryIntent::EpisodicRecall, MemoryLayer::Episode) => 2.1,
            (QueryIntent::EpisodicRecall, MemoryLayer::Belief) => -0.4,
            (QueryIntent::SemanticBackground, MemoryLayer::Procedure) => 1.6,
            (QueryIntent::SemanticBackground, MemoryLayer::Episode) => 0.8,
            (_, MemoryLayer::Trace) => -0.3,
            _ => 0.0,
        }
    }

    fn recency_bonus(hit: &RetrievalHit, memory_layer: MemoryLayer, now: DateTime<Utc>) -> f32 {
        let timestamp = hit.timestamp;
        let age_days = (now.timestamp() - timestamp.timestamp()).max(0) as f32 / 86_400.0;
        match memory_layer {
            MemoryLayer::Episode => (1.1 - (age_days / 21.0)).clamp(0.0, 1.1),
            MemoryLayer::GoalState => (1.0 - (age_days / 14.0)).clamp(0.0, 1.0),
            MemoryLayer::Procedure => (0.25 - (age_days / 120.0)).clamp(0.0, 0.25),
            MemoryLayer::Belief => (0.18 - (age_days / 240.0)).clamp(0.0, 0.18),
            MemoryLayer::SelfModel => (0.12 - (age_days / 365.0)).clamp(0.0, 0.12),
            MemoryLayer::Trace => 0.0,
        }
    }

    fn role_bonus(hit: &RetrievalHit) -> f32 {
        match hit.metadata.get(RETRIEVAL_ROLE_KEY).map(String::as_str) {
            Some("direct_answer") => 0.55,
            Some("support_evidence") => 0.15,
            Some("archive_fallback") => -0.15,
            _ => 0.0,
        }
    }

    fn belief_bonus(intent: QueryIntent, hit: &RetrievalHit) -> f32 {
        if !hit.from_belief {
            return 0.0;
        }

        match intent {
            QueryIntent::CurrentFact => 1.1,
            QueryIntent::HistoricalFact => -0.35,
            _ => 0.2,
        }
    }

    fn goal_state_priority_bonus(hit: &RetrievalHit) -> f32 {
        match hit.metadata.get("goal_status").map(String::as_str) {
            Some("blocked") | Some("waiting_on_user") | Some("waiting_on_system") => 0.8,
            Some("active") => 0.35,
            Some("completed") | Some("inactive") => -1.2,
            _ => 0.0,
        }
    }

    fn procedure_lifecycle_penalty(hit: &RetrievalHit) -> f32 {
        if hit.memory_layer != Some(MemoryLayer::Procedure) {
            return 0.0;
        }

        match hit
            .metadata
            .get("procedure_status")
            .and_then(|value| ProcedureStatus::from_str(value))
        {
            Some(ProcedureStatus::CoolingDown) => -0.75,
            Some(ProcedureStatus::Retired) => -4.0,
            _ => 0.0,
        }
    }
}
