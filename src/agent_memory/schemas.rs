use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::enums::{
    BeliefStatus, BeliefViewStatus, GoalStatus, MemoryLayer, MemoryType, OutcomeFeedbackKind,
    ProcedureStatus, PromotionDecision, QueryIntent, Scope, SelfModelKind, SelfModelStabilityClass,
    SelfModelUpdateRequirement, SourceType,
};
use super::policy::ReasonCode;

const RETRIEVAL_COUNT_KEY: &str = "retrieval_count";
const LAST_ACCESSED_AT_KEY: &str = "last_accessed_at";
const POSITIVE_OUTCOME_COUNT_KEY: &str = "positive_outcome_count";
const NEGATIVE_OUTCOME_COUNT_KEY: &str = "negative_outcome_count";
const LAST_OUTCOME_AT_KEY: &str = "last_outcome_at";
const LAST_POSITIVE_OUTCOME_AT_KEY: &str = "last_positive_outcome_at";
const LAST_NEGATIVE_OUTCOME_AT_KEY: &str = "last_negative_outcome_at";
const OUTCOME_IMPACT_SCORE_KEY: &str = "outcome_impact_score";
const LAST_FEEDBACK_OUTCOME_KEY: &str = "last_feedback_outcome";
const BELIEF_OUTCOME_IMPACT_COUNT_CAP: u32 = 6;
const BELIEF_OUTCOME_IMPACT_WINDOW_DAYS: f32 = 30.0;
const BELIEF_OUTCOME_IMPACT_MAX_ADJUSTMENT: f32 = 0.12;
const BELIEF_OUTCOME_DAY_SECONDS: f32 = 86_400.0;

fn trimmed_non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Provenance metadata attached to a memory candidate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Provenance {
    pub source_type: SourceType,
    pub source_id: String,
    pub source_label: Option<String>,
    pub observed_by: Option<String>,
    pub trust_weight: f32,
}

/// Memory candidate before promotion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidateMemory {
    pub candidate_id: String,
    pub observed_at: DateTime<Utc>,
    /// Entity is intentionally `Option` — absence means the input was not structured enough
    /// to identify a subject. Never substitute a placeholder like "unknown".
    pub entity: Option<String>,
    /// Slot is intentionally `Option` — absence means no typed relationship was asserted.
    /// Never substitute a placeholder like "note".
    pub slot: Option<String>,
    /// Value is intentionally `Option` — absence means the raw text is all we have.
    /// Never copy `raw_text` here to fabricate structure.
    pub value: Option<String>,
    pub raw_text: String,
    pub source: Provenance,
    pub memory_type: MemoryType,
    pub confidence: f32,
    pub salience: f32,
    pub scope: Scope,
    pub ttl: Option<i64>,
    pub event_at: Option<DateTime<Utc>>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    pub internal_layer: Option<MemoryLayer>,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub is_retraction: bool,
}

impl CandidateMemory {
    #[must_use]
    pub fn memory_layer(&self) -> MemoryLayer {
        self.internal_layer
            .unwrap_or_else(|| self.memory_type.memory_layer())
    }

    #[must_use]
    pub fn event_timestamp(&self) -> DateTime<Utc> {
        self.event_at
            .or(self.valid_from)
            .unwrap_or(self.observed_at)
    }

    #[must_use]
    pub fn entity_non_empty(&self) -> Option<&str> {
        self.entity.as_deref().and_then(trimmed_non_empty)
    }

    #[must_use]
    pub fn slot_non_empty(&self) -> Option<&str> {
        self.slot.as_deref().and_then(trimmed_non_empty)
    }

    #[must_use]
    pub fn value_non_empty(&self) -> Option<&str> {
        self.value.as_deref().and_then(trimmed_non_empty)
    }

    #[must_use]
    pub fn workflow_key_non_empty(&self) -> Option<&str> {
        self.metadata
            .get("workflow_key")
            .and_then(|value| trimmed_non_empty(value))
            .or_else(|| self.slot_non_empty())
    }

    #[must_use]
    pub fn has_non_empty_structure(&self) -> bool {
        self.entity_non_empty().is_some()
            || self.slot_non_empty().is_some()
            || self.value_non_empty().is_some()
            || self.workflow_key_non_empty().is_some()
    }

    #[must_use]
    pub fn has_required_structure_for(&self, layer: MemoryLayer) -> bool {
        match layer {
            MemoryLayer::Belief | MemoryLayer::GoalState | MemoryLayer::SelfModel => {
                self.entity_non_empty().is_some()
                    && self.slot_non_empty().is_some()
                    && self.value_non_empty().is_some()
            }
            MemoryLayer::Procedure => {
                self.entity_non_empty().is_some()
                    && self.slot_non_empty().is_some()
                    && self.value_non_empty().is_some()
                    && self.workflow_key_non_empty().is_some()
            }
            MemoryLayer::Episode | MemoryLayer::Trace => true,
        }
    }

    #[must_use]
    pub fn to_episode_memory(&self, stored_at: DateTime<Utc>) -> DurableMemory {
        let mut metadata = self.metadata.clone();
        metadata.insert(
            "source_memory_layer".to_string(),
            self.memory_layer().as_str().to_string(),
        );

        DurableMemory {
            memory_id: uuid::Uuid::new_v4().to_string(),
            candidate_id: self.candidate_id.clone(),
            stored_at,
            updated_at: Some(stored_at),
            // Episodes preserve whatever structure was present; empty string is the honest
            // representation of "no entity asserted" — not the fabricated "unknown".
            entity: self.entity.clone().unwrap_or_default(),
            slot: self.slot.clone().unwrap_or_default(),
            value: self.value.clone().unwrap_or_default(),
            raw_text: self.raw_text.clone(),
            memory_type: MemoryType::Episode,
            confidence: self.confidence,
            salience: self.salience.max(0.55),
            scope: self.scope,
            ttl: None,
            source: self.source.clone(),
            event_at: Some(self.event_timestamp()),
            valid_from: self.valid_from,
            valid_to: self.valid_to,
            internal_layer: Some(MemoryLayer::Episode),
            tags: self.tags.clone(),
            metadata,
            is_retraction: false,
        }
    }
}

/// Durable stored memory record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DurableMemory {
    pub memory_id: String,
    pub candidate_id: String,
    pub stored_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    pub entity: String,
    pub slot: String,
    pub value: String,
    pub raw_text: String,
    pub memory_type: MemoryType,
    pub confidence: f32,
    pub salience: f32,
    pub scope: Scope,
    pub ttl: Option<i64>,
    pub source: Provenance,
    pub event_at: Option<DateTime<Utc>>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    pub internal_layer: Option<MemoryLayer>,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub is_retraction: bool,
}

impl DurableMemory {
    #[must_use]
    pub fn version_timestamp(&self) -> DateTime<Utc> {
        self.updated_at.unwrap_or(self.stored_at)
    }

    #[must_use]
    pub fn memory_layer(&self) -> MemoryLayer {
        self.internal_layer
            .unwrap_or_else(|| self.memory_type.memory_layer())
    }

    #[must_use]
    pub fn event_timestamp(&self) -> DateTime<Utc> {
        self.event_at.or(self.valid_from).unwrap_or(self.stored_at)
    }

    #[must_use]
    pub fn entity_non_empty(&self) -> Option<&str> {
        trimmed_non_empty(&self.entity)
    }

    #[must_use]
    pub fn slot_non_empty(&self) -> Option<&str> {
        trimmed_non_empty(&self.slot)
    }

    #[must_use]
    pub fn value_non_empty(&self) -> Option<&str> {
        trimmed_non_empty(&self.value)
    }

    #[must_use]
    pub fn workflow_key_non_empty(&self) -> Option<&str> {
        self.metadata
            .get("workflow_key")
            .and_then(|value| trimmed_non_empty(value))
            .or_else(|| self.slot_non_empty())
    }

    #[must_use]
    pub fn retrieval_count(&self) -> u32 {
        self.metadata
            .get(RETRIEVAL_COUNT_KEY)
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0)
    }

    #[must_use]
    pub fn last_accessed_at(&self) -> Option<DateTime<Utc>> {
        self.metadata
            .get(LAST_ACCESSED_AT_KEY)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
    }

    #[must_use]
    pub fn with_retrieval_access(mut self, accessed_at: DateTime<Utc>) -> Self {
        self.metadata.insert(
            RETRIEVAL_COUNT_KEY.to_string(),
            self.retrieval_count().saturating_add(1).to_string(),
        );
        self.metadata
            .insert(LAST_ACCESSED_AT_KEY.to_string(), accessed_at.to_rfc3339());
        let current_version_timestamp = self.updated_at.unwrap_or(self.stored_at);
        self.updated_at = Some(current_version_timestamp.max(accessed_at));
        self
    }

    #[must_use]
    pub fn positive_outcome_count(&self) -> u32 {
        self.metadata
            .get(POSITIVE_OUTCOME_COUNT_KEY)
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0)
    }

    #[must_use]
    pub fn negative_outcome_count(&self) -> u32 {
        self.metadata
            .get(NEGATIVE_OUTCOME_COUNT_KEY)
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0)
    }

    #[must_use]
    pub fn last_outcome_at(&self) -> Option<DateTime<Utc>> {
        self.metadata
            .get(LAST_OUTCOME_AT_KEY)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
    }

    #[must_use]
    pub fn outcome_impact_score(&self) -> f32 {
        let positive = self.positive_outcome_count();
        let negative = self.negative_outcome_count();
        let total = positive + negative;
        if total == 0 {
            0.0
        } else {
            ((positive as f32 - negative as f32) / total as f32).clamp(-1.0, 1.0)
        }
    }

    #[must_use]
    pub fn with_outcome_feedback(
        mut self,
        outcome: OutcomeFeedbackKind,
        observed_at: DateTime<Utc>,
    ) -> Self {
        let positive = self.positive_outcome_count();
        let negative = self.negative_outcome_count();
        match outcome {
            OutcomeFeedbackKind::Positive => {
                self.metadata.insert(
                    POSITIVE_OUTCOME_COUNT_KEY.to_string(),
                    positive.saturating_add(1).to_string(),
                );
                self.metadata.insert(
                    LAST_POSITIVE_OUTCOME_AT_KEY.to_string(),
                    observed_at.to_rfc3339(),
                );
            }
            OutcomeFeedbackKind::Negative => {
                self.metadata.insert(
                    NEGATIVE_OUTCOME_COUNT_KEY.to_string(),
                    negative.saturating_add(1).to_string(),
                );
                self.metadata.insert(
                    LAST_NEGATIVE_OUTCOME_AT_KEY.to_string(),
                    observed_at.to_rfc3339(),
                );
            }
        }
        self.metadata
            .insert(LAST_OUTCOME_AT_KEY.to_string(), observed_at.to_rfc3339());
        self.metadata.insert(
            LAST_FEEDBACK_OUTCOME_KEY.to_string(),
            outcome.as_str().to_string(),
        );
        self.metadata.insert(
            OUTCOME_IMPACT_SCORE_KEY.to_string(),
            format!("{:.6}", self.outcome_impact_score()),
        );
        self.updated_at = Some(observed_at);
        self
    }

    #[must_use]
    pub fn has_required_structure_for(&self, layer: MemoryLayer) -> bool {
        match layer {
            MemoryLayer::Belief | MemoryLayer::GoalState | MemoryLayer::SelfModel => {
                self.entity_non_empty().is_some()
                    && self.slot_non_empty().is_some()
                    && self.value_non_empty().is_some()
            }
            MemoryLayer::Procedure => {
                self.entity_non_empty().is_some()
                    && self.slot_non_empty().is_some()
                    && self.value_non_empty().is_some()
                    && self.workflow_key_non_empty().is_some()
            }
            MemoryLayer::Episode | MemoryLayer::Trace => true,
        }
    }

    #[must_use]
    pub fn with_supporting_episode(mut self, episode_id: &str) -> Self {
        let mut supporting_ids: Vec<String> = self
            .metadata
            .get("supporting_episode_ids")
            .map(|value| {
                value
                    .split(',')
                    .filter(|entry| !entry.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if !supporting_ids.iter().any(|existing| existing == episode_id) {
            supporting_ids.push(episode_id.to_string());
        }
        self.metadata.insert(
            "supporting_episode_ids".to_string(),
            supporting_ids.join(","),
        );
        self
    }

    #[must_use]
    pub fn to_episode_record(&self) -> EpisodeRecord {
        EpisodeRecord {
            episode_id: self.memory_id.clone(),
            memory_id: self.memory_id.clone(),
            candidate_id: self.candidate_id.clone(),
            entity: self.entity.clone(),
            slot: self.slot.clone(),
            value: self.value.clone(),
            raw_text: self.raw_text.clone(),
            source: self.source.clone(),
            event_at: self.event_timestamp(),
            stored_at: self.stored_at,
            outcome: self.metadata.get("outcome").cloned(),
            scope: self.scope,
            confidence: self.confidence,
            salience: self.salience,
            tags: self.tags.clone(),
            metadata: self.metadata.clone(),
        }
    }

    #[must_use]
    pub fn to_goal_record(&self) -> Option<GoalRecord> {
        if self.memory_layer() != MemoryLayer::GoalState
            || !self.has_required_structure_for(MemoryLayer::GoalState)
        {
            return None;
        }

        let entity = self.entity_non_empty()?.to_string();
        let slot = self.slot_non_empty()?.to_string();
        let value = self.value_non_empty()?.to_string();

        Some(GoalRecord {
            goal_id: self.memory_id.clone(),
            memory_id: self.memory_id.clone(),
            entity,
            slot,
            value: value.clone(),
            summary: self.raw_text.clone(),
            status: self
                .metadata
                .get("goal_status")
                .and_then(|value| GoalStatus::from_str(value))
                .unwrap_or_else(|| GoalStatus::from_text(&value, &self.raw_text)),
            created_at: self.event_timestamp(),
            updated_at: self.version_timestamp(),
            expires_at: self
                .ttl
                .map(|ttl| self.stored_at + chrono::Duration::seconds(ttl)),
            source: self.source.clone(),
            confidence: self.confidence,
            salience: self.salience,
            supporting_episode_ids: self
                .metadata
                .get("supporting_episode_ids")
                .map(|value| {
                    value
                        .split(',')
                        .filter(|entry| !entry.is_empty())
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            scope: self.scope,
            tags: self.tags.clone(),
            metadata: self.metadata.clone(),
        })
    }

    #[must_use]
    pub fn to_self_model_record(&self) -> Option<SelfModelRecord> {
        if self.memory_layer() != MemoryLayer::SelfModel
            || !self.has_required_structure_for(MemoryLayer::SelfModel)
        {
            return None;
        }

        let entity = self.entity_non_empty()?.to_string();
        let slot = self.slot_non_empty()?.to_string();
        let value = self.value_non_empty()?.to_string();

        Some(SelfModelRecord {
            record_id: self.memory_id.clone(),
            memory_id: self.memory_id.clone(),
            entity,
            slot: slot.clone(),
            value,
            summary: self.raw_text.clone(),
            kind: self
                .metadata
                .get("self_model_kind")
                .and_then(|value| SelfModelKind::from_str(value))
                .unwrap_or_else(|| SelfModelKind::from_slot(&slot)),
            stability_class: self
                .metadata
                .get("self_model_stability_class")
                .and_then(|value| SelfModelStabilityClass::from_str(value))
                .unwrap_or_else(|| {
                    self.metadata
                        .get("self_model_kind")
                        .and_then(|value| SelfModelKind::from_str(value))
                        .unwrap_or_else(|| SelfModelKind::from_slot(&slot))
                        .stability_class()
                }),
            update_requirement: self
                .metadata
                .get("self_model_update_requirement")
                .and_then(|value| SelfModelUpdateRequirement::from_str(value))
                .unwrap_or_else(|| {
                    self.metadata
                        .get("self_model_kind")
                        .and_then(|value| SelfModelKind::from_str(value))
                        .unwrap_or_else(|| SelfModelKind::from_slot(&slot))
                        .update_requirement()
                }),
            status: self
                .metadata
                .get("self_model_status")
                .and_then(|value| BeliefStatus::from_str(value))
                .unwrap_or_else(|| {
                    if self.is_retraction {
                        BeliefStatus::Retracted
                    } else {
                        BeliefStatus::Active
                    }
                }),
            confidence: self.confidence,
            observed_at: self.event_timestamp(),
            updated_at: self.version_timestamp(),
            source: self.source.clone(),
            supporting_memory_ids: self
                .metadata
                .get("supporting_memory_ids")
                .or_else(|| self.metadata.get("supporting_episode_ids"))
                .map(|value| {
                    value
                        .split(',')
                        .filter(|entry| !entry.is_empty())
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_else(|| vec![self.memory_id.clone()]),
            scope: self.scope,
            tags: self.tags.clone(),
            metadata: self.metadata.clone(),
        })
    }

    #[must_use]
    pub fn to_procedure_record(&self) -> Option<ProcedureRecord> {
        if self.memory_layer() != MemoryLayer::Procedure
            || !self.has_required_structure_for(MemoryLayer::Procedure)
        {
            return None;
        }

        let workflow_key = self.workflow_key_non_empty()?;
        let name = self
            .metadata
            .get("procedure_name")
            .and_then(|value| trimmed_non_empty(value))
            .unwrap_or(workflow_key)
            .to_string();

        Some(ProcedureRecord {
            procedure_id: self.memory_id.clone(),
            name,
            description: self.raw_text.clone(),
            context_tags: self
                .metadata
                .get("context_tags")
                .map(|value| {
                    value
                        .split(',')
                        .filter_map(trimmed_non_empty)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_else(|| self.tags.clone()),
            success_count: self
                .metadata
                .get("success_count")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0),
            failure_count: self
                .metadata
                .get("failure_count")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0),
            confidence: self.confidence,
            status: self
                .metadata
                .get("procedure_status")
                .and_then(|value| ProcedureStatus::from_str(value))
                .unwrap_or(ProcedureStatus::Active),
            learned_from_memory_ids: self
                .metadata
                .get("learned_from_memory_ids")
                .map(|value| {
                    value
                        .split(',')
                        .filter_map(trimmed_non_empty)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            last_used_at: self
                .metadata
                .get("last_used_at")
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc)),
            last_succeeded_at: self
                .metadata
                .get("last_succeeded_at")
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc)),
            last_failed_at: self
                .metadata
                .get("last_failed_at")
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc)),
            updated_at: self.version_timestamp(),
            metadata: self.metadata.clone(),
        })
    }
}

/// Important time-stamped event retained for historical reasoning.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpisodeRecord {
    pub episode_id: String,
    pub memory_id: String,
    pub candidate_id: String,
    pub entity: String,
    pub slot: String,
    pub value: String,
    pub raw_text: String,
    pub source: Provenance,
    pub event_at: DateTime<Utc>,
    pub stored_at: DateTime<Utc>,
    pub outcome: Option<String>,
    pub scope: Scope,
    pub confidence: f32,
    pub salience: f32,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
}

/// Current or recently active task-state memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GoalRecord {
    pub goal_id: String,
    pub memory_id: String,
    pub entity: String,
    pub slot: String,
    pub value: String,
    pub summary: String,
    pub status: GoalStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub source: Provenance,
    pub confidence: f32,
    pub salience: f32,
    pub supporting_episode_ids: Vec<String>,
    pub scope: Scope,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
}

/// Durable user or agent operating preference grounded in evidence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SelfModelRecord {
    pub record_id: String,
    pub memory_id: String,
    pub entity: String,
    pub slot: String,
    pub value: String,
    pub summary: String,
    pub kind: SelfModelKind,
    pub stability_class: SelfModelStabilityClass,
    pub update_requirement: SelfModelUpdateRequirement,
    pub status: BeliefStatus,
    pub confidence: f32,
    pub observed_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub source: Provenance,
    pub supporting_memory_ids: Vec<String>,
    pub scope: Scope,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
}

/// Reusable operational pattern learned from repeated success.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcedureRecord {
    pub procedure_id: String,
    pub name: String,
    pub description: String,
    pub context_tags: Vec<String>,
    pub success_count: u32,
    pub failure_count: u32,
    pub confidence: f32,
    pub status: ProcedureStatus,
    pub learned_from_memory_ids: Vec<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub last_succeeded_at: Option<DateTime<Utc>>,
    pub last_failed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub metadata: BTreeMap<String, String>,
}

/// Evidence that repeated episodes caused a bounded promotion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConsolidationRecord {
    pub consolidation_id: String,
    pub target_layer: MemoryLayer,
    pub target_id: Option<String>,
    pub source_memory_ids: Vec<String>,
    pub reason: String,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
    pub metadata: BTreeMap<String, String>,
}

/// Explicit current belief state with supporting history references.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BeliefRecord {
    pub belief_id: String,
    pub entity: String,
    pub slot: String,
    pub current_value: String,
    pub status: BeliefStatus,
    pub confidence: f32,
    pub valid_from: DateTime<Utc>,
    pub valid_to: Option<DateTime<Utc>>,
    pub last_reviewed_at: DateTime<Utc>,
    pub supporting_memory_ids: Vec<String>,
    pub opposing_memory_ids: Vec<String>,
    #[serde(default)]
    pub contradictions_observed: u32,
    #[serde(default)]
    pub last_contradiction_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub time_to_last_resolution_seconds: Option<i64>,
    #[serde(default)]
    pub positive_outcome_count: u32,
    #[serde(default)]
    pub negative_outcome_count: u32,
    #[serde(default)]
    pub last_outcome_at: Option<DateTime<Utc>>,
    pub source_weights: BTreeMap<SourceType, f32>,
}

impl BeliefRecord {
    #[must_use]
    pub fn strongest_source_weight(&self) -> f32 {
        self.source_weights
            .values()
            .copied()
            .fold(0.0_f32, f32::max)
    }

    #[must_use]
    pub const fn view_status(&self) -> BeliefViewStatus {
        self.status.view_status()
    }

    #[must_use]
    pub fn time_in_dispute_seconds(&self, now: DateTime<Utc>) -> Option<i64> {
        if self.status != BeliefStatus::Disputed {
            return None;
        }
        self.last_contradiction_at
            .map(|observed_at| (now.timestamp() - observed_at.timestamp()).max(0))
    }

    #[must_use]
    pub fn outcome_impact_score(&self) -> f32 {
        let total = self.positive_outcome_count + self.negative_outcome_count;
        if total == 0 {
            0.0
        } else {
            ((self.positive_outcome_count as f32 - self.negative_outcome_count as f32)
                / total as f32)
                .clamp(-1.0, 1.0)
        }
    }

    #[must_use]
    pub fn effective_confidence(&self, now: DateTime<Utc>) -> f32 {
        (self.confidence + self.outcome_impact_adjustment(now)).clamp(0.05, 0.99)
    }

    #[must_use]
    pub fn outcome_impact_adjustment(&self, now: DateTime<Utc>) -> f32 {
        let total = self.positive_outcome_count + self.negative_outcome_count;
        if total == 0 {
            return 0.0;
        }

        let count_weight = total.min(BELIEF_OUTCOME_IMPACT_COUNT_CAP) as f32
            / BELIEF_OUTCOME_IMPACT_COUNT_CAP as f32;
        let recency_weight = self.last_outcome_at.map_or(0.0, |last_outcome_at| {
            let age_days = (now.timestamp() - last_outcome_at.timestamp()).max(0) as f32
                / BELIEF_OUTCOME_DAY_SECONDS;
            (1.0 - (age_days / BELIEF_OUTCOME_IMPACT_WINDOW_DAYS)).clamp(0.0, 1.0)
        });

        self.outcome_impact_score()
            * count_weight
            * recency_weight
            * BELIEF_OUTCOME_IMPACT_MAX_ADJUSTMENT
    }

    #[must_use]
    pub fn with_outcome_feedback(
        mut self,
        outcome: OutcomeFeedbackKind,
        observed_at: DateTime<Utc>,
    ) -> Self {
        match outcome {
            OutcomeFeedbackKind::Positive => {
                self.positive_outcome_count = self.positive_outcome_count.saturating_add(1);
            }
            OutcomeFeedbackKind::Negative => {
                self.negative_outcome_count = self.negative_outcome_count.saturating_add(1);
            }
        }
        self.last_outcome_at = Some(observed_at);
        self.last_reviewed_at = observed_at;
        self
    }
}

/// Type-specific retention policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetentionRule {
    pub memory_layer: MemoryLayer,
    pub memory_type: MemoryType,
    pub default_ttl: Option<i64>,
    pub decay_per_day: f32,
    pub retrieval_priority: f32,
    pub promotable: bool,
}

/// Retrieval request shaped by task intent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetrievalQuery {
    pub query_text: String,
    pub intent: QueryIntent,
    pub entity: Option<String>,
    pub slot: Option<String>,
    pub scope: Option<Scope>,
    pub top_k: usize,
    pub as_of: Option<DateTime<Utc>>,
    pub include_expired: bool,
}

/// External outcome signal attached to a memory id or workflow key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutcomeFeedback {
    pub memory_id: Option<String>,
    pub belief_id: Option<String>,
    pub workflow_key: Option<String>,
    pub outcome: OutcomeFeedbackKind,
    pub observed_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

/// Retrieval result emitted by the governed layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetrievalHit {
    pub memory_id: Option<String>,
    pub belief_id: Option<String>,
    pub entity: Option<String>,
    pub slot: Option<String>,
    pub value: Option<String>,
    pub text: String,
    pub memory_layer: Option<MemoryLayer>,
    pub memory_type: Option<MemoryType>,
    pub score: f32,
    pub timestamp: DateTime<Utc>,
    pub scope: Option<Scope>,
    pub source: Option<SourceType>,
    pub from_belief: bool,
    pub expired: bool,
    pub metadata: BTreeMap<String, String>,
}

/// Context gathered before deciding whether a candidate may enter a durable layer.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PromotionContext {
    pub belief_evidence_count: usize,
    pub self_model_evidence_count: usize,
    pub goal_state_evidence_count: usize,
    pub procedure_success_count: usize,
    pub procedure_failure_count: usize,
    pub corroborating_evidence_count: usize,
    pub contradictory_evidence_count: usize,
    pub verified_source: bool,
    pub seeded_by_system: bool,
}

/// Promotion outcome for a candidate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromotionResult {
    pub decision: PromotionDecision,
    pub score: f32,
    pub reason: String,
    pub reason_code: Option<ReasonCode>,
    pub durable_memory: Option<DurableMemory>,
    pub details: BTreeMap<String, String>,
}

/// Append-only audit event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditEvent {
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub action: String,
    pub candidate_id: Option<String>,
    pub memory_id: Option<String>,
    pub belief_id: Option<String>,
    pub query_text: Option<String>,
    pub details: BTreeMap<String, String>,
}
