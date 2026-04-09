#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use memvid_core::agent_memory::adapters::memvid_store::InMemoryMemoryStore;
use memvid_core::agent_memory::audit::{AuditLogger, InMemoryAuditSink};
use memvid_core::agent_memory::belief_updater::BeliefUpdater;
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::{MemoryType, Scope, SourceType};
use memvid_core::agent_memory::memory_classifier::MemoryClassifier;
use memvid_core::agent_memory::memory_controller::MemoryController;
use memvid_core::agent_memory::memory_promoter::MemoryPromoter;
use memvid_core::agent_memory::memory_retriever::MemoryRetriever;
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::ranker::Ranker;
use memvid_core::agent_memory::retention::RetentionManager;
use memvid_core::agent_memory::schemas::{CandidateMemory, DurableMemory, Provenance};

pub fn ts(seconds: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(seconds, 0).expect("valid timestamp")
}

pub fn candidate(entity: &str, slot: &str, value: &str, raw_text: &str) -> CandidateMemory {
    CandidateMemory {
        candidate_id: format!("candidate-{entity}-{slot}-{value}"),
        observed_at: ts(1_700_000_000),
        entity: if entity.is_empty() {
            None
        } else {
            Some(entity.to_string())
        },
        slot: if slot.is_empty() {
            None
        } else {
            Some(slot.to_string())
        },
        value: if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        },
        raw_text: raw_text.to_string(),
        source: Provenance {
            source_type: SourceType::Chat,
            source_id: "chat:1".to_string(),
            source_label: Some("chat".to_string()),
            observed_by: None,
            trust_weight: 0.75,
        },
        memory_type: MemoryType::Trace,
        confidence: 0.92,
        salience: 0.88,
        scope: Scope::Private,
        ttl: None,
        event_at: None,
        valid_from: None,
        valid_to: None,
        internal_layer: None,
        tags: Vec::new(),
        metadata: BTreeMap::new(),
        is_retraction: false,
    }
}

pub fn durable(
    entity: &str,
    slot: &str,
    value: &str,
    raw_text: &str,
    memory_type: MemoryType,
    source_type: SourceType,
    trust_weight: f32,
    stored_at: DateTime<Utc>,
) -> DurableMemory {
    DurableMemory {
        memory_id: format!("memory-{entity}-{slot}-{value}-{}", stored_at.timestamp()),
        candidate_id: format!("candidate-{entity}-{slot}-{value}"),
        stored_at,
        updated_at: Some(stored_at),
        entity: entity.to_string(),
        slot: slot.to_string(),
        value: value.to_string(),
        raw_text: raw_text.to_string(),
        memory_type,
        confidence: 0.9,
        salience: 0.8,
        scope: Scope::Private,
        ttl: None,
        source: Provenance {
            source_type,
            source_id: format!("source-{entity}-{slot}"),
            source_label: None,
            observed_by: None,
            trust_weight,
        },
        event_at: None,
        valid_from: None,
        valid_to: None,
        internal_layer: None,
        tags: Vec::new(),
        metadata: BTreeMap::new(),
        is_retraction: false,
    }
}

pub fn candidate_from_durable(memory: &DurableMemory) -> CandidateMemory {
    CandidateMemory {
        candidate_id: memory.candidate_id.clone(),
        observed_at: memory.event_at.unwrap_or(memory.stored_at),
        entity: (!memory.entity.is_empty()).then(|| memory.entity.clone()),
        slot: (!memory.slot.is_empty()).then(|| memory.slot.clone()),
        value: (!memory.value.is_empty()).then(|| memory.value.clone()),
        raw_text: memory.raw_text.clone(),
        source: memory.source.clone(),
        memory_type: memory.memory_type,
        confidence: memory.confidence,
        salience: memory.salience,
        scope: memory.scope,
        ttl: memory.ttl,
        event_at: memory.event_at,
        valid_from: memory.valid_from,
        valid_to: memory.valid_to,
        internal_layer: memory.internal_layer,
        tags: memory.tags.clone(),
        metadata: memory.metadata.clone(),
        is_retraction: memory.is_retraction,
    }
}

pub fn ingest_durable(
    controller: &mut MemoryController<InMemoryMemoryStore>,
    memory: &DurableMemory,
) -> Option<String> {
    controller
        .ingest(candidate_from_durable(memory))
        .expect("ingest succeeds")
}

pub fn apply_durable(
    controller: &mut MemoryController<InMemoryMemoryStore>,
    memory: &DurableMemory,
    supporting_episode_id: Option<&str>,
) -> String {
    controller
        .apply_durable_memory(memory.clone(), supporting_episode_id)
        .expect("durable memory applied")
}

pub fn controller(
    now: DateTime<Utc>,
) -> (MemoryController<InMemoryMemoryStore>, InMemoryAuditSink) {
    controller_with_policy(now, PolicySet::default())
}

pub fn controller_with_policy(
    now: DateTime<Utc>,
    policy: PolicySet,
) -> (MemoryController<InMemoryMemoryStore>, InMemoryAuditSink) {
    let sink = InMemoryAuditSink::default();
    let clock = Arc::new(FixedClock::new(now));
    let controller = MemoryController::new(
        InMemoryMemoryStore::default(),
        clock.clone(),
        AuditLogger::new(clock.clone(), Arc::new(sink.clone())),
        MemoryClassifier,
        MemoryPromoter::new(policy.clone()),
        BeliefUpdater,
        MemoryRetriever::new(Ranker, RetentionManager::new(policy)),
    );
    (controller, sink)
}
