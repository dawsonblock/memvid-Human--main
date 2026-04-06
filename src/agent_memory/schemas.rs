use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::enums::{
    BeliefStatus, GoalStatus, MemoryLayer, MemoryType, ProcedureStatus, PromotionDecision,
    QueryIntent, Scope, SelfModelKind, SourceType,
};

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
    pub entity: String,
    pub slot: String,
    pub value: String,
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
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub is_retraction: bool,
}

impl CandidateMemory {
    #[must_use]
    pub fn memory_layer(&self) -> MemoryLayer {
        self.memory_type.memory_layer()
    }

    #[must_use]
    pub fn event_timestamp(&self) -> DateTime<Utc> {
        self.event_at
            .or(self.valid_from)
            .unwrap_or(self.observed_at)
    }
}

/// Durable stored memory record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DurableMemory {
    pub memory_id: String,
    pub candidate_id: String,
    pub stored_at: DateTime<Utc>,
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
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub is_retraction: bool,
}

impl DurableMemory {
    #[must_use]
    pub fn memory_layer(&self) -> MemoryLayer {
        self.memory_type.memory_layer()
    }

    #[must_use]
    pub fn event_timestamp(&self) -> DateTime<Utc> {
        self.event_at.or(self.valid_from).unwrap_or(self.stored_at)
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
        if self.memory_layer() != MemoryLayer::GoalState {
            return None;
        }

        Some(GoalRecord {
            goal_id: self.memory_id.clone(),
            memory_id: self.memory_id.clone(),
            entity: self.entity.clone(),
            slot: self.slot.clone(),
            value: self.value.clone(),
            summary: self.raw_text.clone(),
            status: GoalStatus::from_text(&self.value, &self.raw_text),
            created_at: self.event_timestamp(),
            updated_at: self.stored_at,
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
        if self.memory_layer() != MemoryLayer::SelfModel {
            return None;
        }

        Some(SelfModelRecord {
            record_id: self.memory_id.clone(),
            memory_id: self.memory_id.clone(),
            entity: self.entity.clone(),
            slot: self.slot.clone(),
            value: self.value.clone(),
            summary: self.raw_text.clone(),
            kind: SelfModelKind::from_slot(&self.slot),
            status: if self.is_retraction {
                BeliefStatus::Retracted
            } else {
                BeliefStatus::Active
            },
            confidence: self.confidence,
            observed_at: self.event_timestamp(),
            updated_at: self.stored_at,
            source: self.source.clone(),
            supporting_memory_ids: vec![self.memory_id.clone()],
            scope: self.scope,
            tags: self.tags.clone(),
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
    pub source_weights: BTreeMap<SourceType, f32>,
}

/// Type-specific retention policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetentionRule {
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

/// Retrieval result emitted by the governed layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetrievalHit {
    pub memory_id: Option<String>,
    pub belief_id: Option<String>,
    pub entity: Option<String>,
    pub slot: Option<String>,
    pub value: Option<String>,
    pub text: String,
    pub memory_type: Option<MemoryType>,
    pub score: f32,
    pub timestamp: DateTime<Utc>,
    pub scope: Option<Scope>,
    pub source: Option<SourceType>,
    pub from_belief: bool,
    pub expired: bool,
    pub metadata: BTreeMap<String, String>,
}

/// Promotion outcome for a candidate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromotionResult {
    pub decision: PromotionDecision,
    pub score: f32,
    pub reason: String,
    pub durable_memory: Option<DurableMemory>,
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
