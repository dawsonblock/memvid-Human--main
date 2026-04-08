use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::enums::{MemoryLayer, MemoryType, ProcedureStatus, QueryIntent};
use super::policy::{PolicyProfile, SoftWeights};
use super::schemas::RetrievalHit;

const RETRIEVAL_ROLE_KEY: &str = "retrieval_role";
const CONTENT_MATCH_SCALE: f32 = 3.0;
const GOAL_RELEVANCE_SCALE: f32 = 2.0;
const SELF_RELEVANCE_SCALE: f32 = 2.0;
const SALIENCE_SCALE: f32 = 1.5;
const EVIDENCE_STRENGTH_SCALE: f32 = 2.0;
const CONTRADICTION_PENALTY_SCALE: f32 = 5.0;
const RECENCY_SCALE: f32 = 3.0;
const PROCEDURE_SUCCESS_SCALE: f32 = 3.0;

pub(crate) const SCORE_COMPONENTS_KEY: &str = "score_components";
pub(crate) const RANKING_EXPLANATION_KEY: &str = "ranking_explanation";
pub(crate) const SCORE_COMPONENT_LAYER_MATCH_KEY: &str = "score_component_layer_match";
pub(crate) const SCORE_COMPONENT_ROLE_PRIORITY_KEY: &str = "score_component_role_priority";
pub(crate) const SCORE_COMPONENT_BELIEF_PRIORITY_KEY: &str = "score_component_belief_priority";
pub(crate) const SCORE_COMPONENT_CONTENT_MATCH_KEY: &str = "score_component_content_match";
pub(crate) const SCORE_COMPONENT_GOAL_RELEVANCE_KEY: &str = "score_component_goal_relevance";
pub(crate) const SCORE_COMPONENT_SELF_RELEVANCE_KEY: &str = "score_component_self_relevance";
pub(crate) const SCORE_COMPONENT_SALIENCE_KEY: &str = "score_component_salience";
pub(crate) const SCORE_COMPONENT_EVIDENCE_STRENGTH_KEY: &str = "score_component_evidence_strength";
pub(crate) const SCORE_COMPONENT_CONTRADICTION_PENALTY_KEY: &str =
    "score_component_contradiction_penalty";
pub(crate) const SCORE_COMPONENT_RECENCY_KEY: &str = "score_component_recency";
pub(crate) const SCORE_COMPONENT_PROCEDURE_SUCCESS_KEY: &str = "score_component_procedure_success";
pub(crate) const SCORE_COMPONENT_GOAL_STATUS_PRIORITY_KEY: &str =
    "score_component_goal_status_priority";
pub(crate) const SCORE_COMPONENT_LIFECYCLE_HISTORY_BONUS_KEY: &str =
    "score_component_lifecycle_history_bonus";
pub(crate) const SCORE_COMPONENT_PROCEDURE_LIFECYCLE_PENALTY_KEY: &str =
    "score_component_procedure_lifecycle_penalty";
pub(crate) const SCORE_COMPONENT_EXPIRY_PENALTY_KEY: &str = "score_component_expiry_penalty";
pub(crate) const SCORE_COMPONENT_TOTAL_KEY: &str = "score_component_total";
pub(crate) const SCORE_SIGNAL_CONTENT_MATCH_KEY: &str = "score_signal_content_match";
pub(crate) const SCORE_SIGNAL_GOAL_RELEVANCE_KEY: &str = "score_signal_goal_relevance";
pub(crate) const SCORE_SIGNAL_SELF_RELEVANCE_KEY: &str = "score_signal_self_relevance";
pub(crate) const SCORE_SIGNAL_SALIENCE_KEY: &str = "score_signal_salience";
pub(crate) const SCORE_SIGNAL_EVIDENCE_STRENGTH_KEY: &str = "score_signal_evidence_strength";
pub(crate) const SCORE_SIGNAL_CONTRADICTION_KEY: &str = "score_signal_contradiction_penalty";
pub(crate) const SCORE_SIGNAL_PROCEDURE_SUCCESS_KEY: &str = "score_signal_procedure_success";

#[derive(Debug, Clone, Copy, Default)]
struct ScoreBreakdown {
    layer_match: f32,
    role_priority: f32,
    belief_priority: f32,
    content_match: f32,
    goal_relevance: f32,
    self_relevance: f32,
    salience: f32,
    evidence_strength: f32,
    contradiction_penalty: f32,
    recency: f32,
    procedure_success: f32,
    goal_status_priority: f32,
    lifecycle_history_bonus: f32,
    procedure_lifecycle_penalty: f32,
    expiry_penalty: f32,
}

impl ScoreBreakdown {
    fn total(self) -> f32 {
        self.layer_match
            + self.role_priority
            + self.belief_priority
            + self.content_match
            + self.goal_relevance
            + self.self_relevance
            + self.salience
            + self.evidence_strength
            + self.contradiction_penalty
            + self.recency
            + self.procedure_success
            + self.goal_status_priority
            + self.lifecycle_history_bonus
            + self.procedure_lifecycle_penalty
            + self.expiry_penalty
    }

    fn ordered_components(self) -> [(&'static str, f32); 16] {
        [
            (SCORE_COMPONENT_LAYER_MATCH_KEY, self.layer_match),
            (SCORE_COMPONENT_ROLE_PRIORITY_KEY, self.role_priority),
            (SCORE_COMPONENT_BELIEF_PRIORITY_KEY, self.belief_priority),
            (SCORE_COMPONENT_CONTENT_MATCH_KEY, self.content_match),
            (SCORE_COMPONENT_GOAL_RELEVANCE_KEY, self.goal_relevance),
            (SCORE_COMPONENT_SELF_RELEVANCE_KEY, self.self_relevance),
            (SCORE_COMPONENT_SALIENCE_KEY, self.salience),
            (
                SCORE_COMPONENT_EVIDENCE_STRENGTH_KEY,
                self.evidence_strength,
            ),
            (
                SCORE_COMPONENT_CONTRADICTION_PENALTY_KEY,
                self.contradiction_penalty,
            ),
            (SCORE_COMPONENT_RECENCY_KEY, self.recency),
            (
                SCORE_COMPONENT_PROCEDURE_SUCCESS_KEY,
                self.procedure_success,
            ),
            (
                SCORE_COMPONENT_GOAL_STATUS_PRIORITY_KEY,
                self.goal_status_priority,
            ),
            (
                SCORE_COMPONENT_LIFECYCLE_HISTORY_BONUS_KEY,
                self.lifecycle_history_bonus,
            ),
            (
                SCORE_COMPONENT_PROCEDURE_LIFECYCLE_PENALTY_KEY,
                self.procedure_lifecycle_penalty,
            ),
            (SCORE_COMPONENT_EXPIRY_PENALTY_KEY, self.expiry_penalty),
            (SCORE_COMPONENT_TOTAL_KEY, self.total()),
        ]
    }
}

/// Deterministic reranker for governed retrieval hits.
#[derive(Debug, Default, Clone, Copy)]
pub struct Ranker;

impl Ranker {
    #[must_use]
    pub fn rerank(
        &self,
        hits: Vec<RetrievalHit>,
        intent: QueryIntent,
        now: DateTime<Utc>,
    ) -> Vec<RetrievalHit> {
        let policy_profile = PolicyProfile::default();
        self.rerank_with_weights(hits, intent, now, policy_profile.soft_weights())
    }

    #[must_use]
    pub fn rerank_with_weights(
        &self,
        mut hits: Vec<RetrievalHit>,
        intent: QueryIntent,
        now: DateTime<Utc>,
        weights: &SoftWeights,
    ) -> Vec<RetrievalHit> {
        for hit in &mut hits {
            let breakdown = Self::score_breakdown(hit, intent, now, weights);
            hit.score = breakdown.total();
            Self::record_breakdown(hit, intent, breakdown);
        }
        hits.sort_by(|left, right| right.score.total_cmp(&left.score));
        hits
    }

    fn score_breakdown(
        hit: &RetrievalHit,
        intent: QueryIntent,
        now: DateTime<Utc>,
        weights: &SoftWeights,
    ) -> ScoreBreakdown {
        let memory_layer = hit
            .memory_layer
            .or_else(|| hit.memory_type.map(MemoryType::memory_layer));
        let layer_match = memory_layer
            .map(|layer| Self::type_bonus(intent, hit, layer))
            .unwrap_or(0.0);
        let recency = memory_layer
            .map(|layer| Self::recency_signal(hit, layer, now) * weights.recency * RECENCY_SCALE)
            .unwrap_or(0.0);

        ScoreBreakdown {
            layer_match,
            role_priority: Self::role_bonus(hit),
            belief_priority: Self::belief_bonus(intent, hit),
            content_match: Self::signal_value(hit, SCORE_SIGNAL_CONTENT_MATCH_KEY)
                .unwrap_or(hit.score)
                * weights.content_match
                * CONTENT_MATCH_SCALE,
            goal_relevance: Self::goal_relevance_signal(hit)
                * weights.goal_relevance
                * GOAL_RELEVANCE_SCALE,
            self_relevance: Self::self_relevance_signal(hit)
                * weights.self_relevance
                * SELF_RELEVANCE_SCALE,
            salience: Self::signal_value(hit, SCORE_SIGNAL_SALIENCE_KEY).unwrap_or(0.0)
                * weights.salience
                * SALIENCE_SCALE,
            evidence_strength: Self::signal_value(hit, SCORE_SIGNAL_EVIDENCE_STRENGTH_KEY)
                .unwrap_or(0.0)
                * weights.evidence_strength
                * EVIDENCE_STRENGTH_SCALE,
            contradiction_penalty: -Self::contradiction_signal(hit)
                * weights.contradiction_penalty
                * CONTRADICTION_PENALTY_SCALE,
            recency,
            procedure_success: Self::procedure_success_signal(hit)
                * weights.procedure_success
                * PROCEDURE_SUCCESS_SCALE,
            goal_status_priority: Self::goal_state_priority_bonus(hit),
            lifecycle_history_bonus: Self::lifecycle_history_bonus(hit),
            procedure_lifecycle_penalty: Self::procedure_lifecycle_penalty(hit),
            expiry_penalty: if hit.expired { -1.0 } else { 0.0 },
        }
    }

    fn type_bonus(intent: QueryIntent, hit: &RetrievalHit, memory_layer: MemoryLayer) -> f32 {
        match (intent, memory_layer) {
            (QueryIntent::CurrentFact, MemoryLayer::Belief) => {
                if hit.from_belief {
                    2.8
                } else {
                    1.35
                }
            }
            (QueryIntent::CurrentFact, MemoryLayer::Episode) => 0.45,
            (QueryIntent::CurrentFact, MemoryLayer::Trace) => -0.8,
            (QueryIntent::HistoricalFact, MemoryLayer::Episode) => 2.0,
            (QueryIntent::HistoricalFact, MemoryLayer::Belief) => 1.1,
            (QueryIntent::HistoricalFact, MemoryLayer::Trace) => -0.6,
            (QueryIntent::PreferenceLookup, MemoryLayer::SelfModel) => 2.2,
            (QueryIntent::PreferenceLookup, MemoryLayer::Episode) => 0.6,
            (QueryIntent::PreferenceLookup, MemoryLayer::Trace) => -0.6,
            (QueryIntent::TaskState, MemoryLayer::GoalState) => 2.4,
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

    fn recency_signal(hit: &RetrievalHit, memory_layer: MemoryLayer, now: DateTime<Utc>) -> f32 {
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
            QueryIntent::CurrentFact => 1.35,
            QueryIntent::HistoricalFact => -0.35,
            _ => 0.2,
        }
    }

    fn goal_state_priority_bonus(hit: &RetrievalHit) -> f32 {
        match hit.metadata.get("goal_status").map(String::as_str) {
            Some("blocked" | "waiting_on_user" | "waiting_on_system") => 0.8,
            Some("active") => 0.35,
            Some("completed" | "inactive") => -1.2,
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
            Some(ProcedureStatus::CoolingDown) => -1.5,
            Some(ProcedureStatus::Retired) => -4.0,
            _ => 0.0,
        }
    }

    fn lifecycle_history_bonus(hit: &RetrievalHit) -> f32 {
        if hit.memory_layer == Some(MemoryLayer::Trace)
            && hit.metadata.get("action").map(String::as_str) == Some("procedure_status_changed")
            && hit.metadata.get(RETRIEVAL_ROLE_KEY).map(String::as_str) == Some("direct_answer")
        {
            0.75
        } else {
            0.0
        }
    }

    fn goal_relevance_signal(hit: &RetrievalHit) -> f32 {
        Self::signal_value(hit, SCORE_SIGNAL_GOAL_RELEVANCE_KEY).unwrap_or(0.0)
    }

    fn self_relevance_signal(hit: &RetrievalHit) -> f32 {
        Self::signal_value(hit, SCORE_SIGNAL_SELF_RELEVANCE_KEY).unwrap_or(0.0)
    }

    fn contradiction_signal(hit: &RetrievalHit) -> f32 {
        Self::signal_value(hit, SCORE_SIGNAL_CONTRADICTION_KEY).unwrap_or_else(|| {
            match hit
                .metadata
                .get("belief_retrieval_status")
                .map(String::as_str)
            {
                Some("contested") => 1.0,
                Some("superseded") => 1.6,
                Some("retracted") => 2.0,
                _ => 0.0,
            }
        })
    }

    fn procedure_success_signal(hit: &RetrievalHit) -> f32 {
        if let Some(signal) = Self::signal_value(hit, SCORE_SIGNAL_PROCEDURE_SUCCESS_KEY) {
            return signal;
        }

        let success_count = hit
            .metadata
            .get("success_count")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let failure_count = hit
            .metadata
            .get("failure_count")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let total_runs = success_count + failure_count;
        if total_runs == 0 {
            0.0
        } else {
            success_count as f32 / total_runs as f32
        }
    }

    fn signal_value(hit: &RetrievalHit, key: &str) -> Option<f32> {
        hit.metadata
            .get(key)
            .and_then(|value| value.parse::<f32>().ok())
    }

    fn record_breakdown(hit: &mut RetrievalHit, intent: QueryIntent, breakdown: ScoreBreakdown) {
        let mut component_map = BTreeMap::new();
        for (key, value) in breakdown.ordered_components() {
            let formatted = format!("{value:.6}");
            hit.metadata.insert(key.to_string(), formatted.clone());
            component_map.insert(key, formatted);
        }
        hit.metadata.insert(
            SCORE_COMPONENTS_KEY.to_string(),
            component_map
                .into_iter()
                .map(|(key, value)| {
                    (
                        key.trim_start_matches("score_component_").to_string(),
                        value,
                    )
                })
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("|"),
        );
        hit.metadata.insert(
            RANKING_EXPLANATION_KEY.to_string(),
            Self::ranking_explanation(hit, intent, breakdown),
        );
    }

    fn ranking_explanation(
        hit: &RetrievalHit,
        intent: QueryIntent,
        breakdown: ScoreBreakdown,
    ) -> String {
        let role = hit
            .metadata
            .get(RETRIEVAL_ROLE_KEY)
            .map(String::as_str)
            .unwrap_or("unclassified");
        let layer = hit
            .memory_layer
            .or_else(|| hit.memory_type.map(MemoryType::memory_layer))
            .map(MemoryLayer::as_str)
            .unwrap_or("unknown");

        let mut strongest = breakdown
            .ordered_components()
            .into_iter()
            .filter(|(key, _)| *key != SCORE_COMPONENT_TOTAL_KEY)
            .filter(|(_, value)| value.abs() >= 0.01)
            .map(|(key, value)| {
                (
                    key.trim_start_matches("score_component_").replace('_', " "),
                    value,
                )
            })
            .collect::<Vec<_>>();
        strongest.sort_by(|left, right| right.1.abs().total_cmp(&left.1.abs()));
        strongest.truncate(3);

        let summary = strongest
            .into_iter()
            .map(|(label, value)| format!("{label}={value:.2}"))
            .collect::<Vec<_>>()
            .join(", ");

        let intent_label = match intent {
            QueryIntent::CurrentFact => "current_fact",
            QueryIntent::HistoricalFact => "historical_fact",
            QueryIntent::PreferenceLookup => "preference_lookup",
            QueryIntent::TaskState => "task_state",
            QueryIntent::EpisodicRecall => "episodic_recall",
            QueryIntent::SemanticBackground => "semantic_background",
        };

        if summary.is_empty() {
            format!("{role} {layer} hit for {intent_label}")
        } else {
            format!("{role} {layer} hit for {intent_label}; strongest factors: {summary}")
        }
    }
}
