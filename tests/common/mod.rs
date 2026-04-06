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
        entity: entity.to_string(),
        slot: slot.to_string(),
        value: value.to_string(),
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

pub fn controller(
    now: DateTime<Utc>,
) -> (MemoryController<InMemoryMemoryStore>, InMemoryAuditSink) {
    let sink = InMemoryAuditSink::default();
    let clock = Arc::new(FixedClock::new(now));
    let policy = PolicySet::default();
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
