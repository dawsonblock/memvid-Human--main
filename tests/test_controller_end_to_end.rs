mod common;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use memvid_core::agent_memory::adapters::memvid_store::MemoryStore;
use memvid_core::agent_memory::audit::{AuditLogger, InMemoryAuditSink};
use memvid_core::agent_memory::belief_updater::BeliefUpdater;
use memvid_core::agent_memory::clock::FixedClock;
use memvid_core::agent_memory::enums::OutcomeFeedbackKind;
use memvid_core::agent_memory::enums::{MemoryType, QueryIntent, SourceType};
use memvid_core::agent_memory::errors::Result;
use memvid_core::agent_memory::memory_classifier::MemoryClassifier;
use memvid_core::agent_memory::memory_controller::MemoryController;
use memvid_core::agent_memory::memory_promoter::MemoryPromoter;
use memvid_core::agent_memory::memory_retriever::MemoryRetriever;
use memvid_core::agent_memory::policy::PolicySet;
use memvid_core::agent_memory::ranker::Ranker;
use memvid_core::agent_memory::retention::RetentionManager;
use memvid_core::agent_memory::schemas::{
    BeliefRecord, DurableMemory, OutcomeFeedback, RetrievalHit, RetrievalQuery,
};

use common::{apply_durable, candidate, controller, controller_with_policy, durable, ts};

#[test]
fn ingest_low_trust_fact_preserves_episode_evidence_without_promoting_truth() {
    let (mut controller, sink) = controller(ts(1_700_000_000));

    let memory_id = controller
        .ingest(candidate(
            "user",
            "location",
            "Berlin",
            "The user currently lives in Berlin.",
        ))
        .expect("ingest succeeds")
        .expect("episode evidence stored");

    assert_eq!(controller.store().memories().len(), 1);
    assert_eq!(controller.store().beliefs().len(), 0);
    assert!(!memory_id.is_empty());
    assert_eq!(
        controller.store().memories()[0].memory_layer().as_str(),
        "episode"
    );

    let promotion_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "promotion")
        .expect("promotion audit event present");
    assert_eq!(
        promotion_event.details.get("reason").map(String::as_str),
        Some("belief promotion requires repeated evidence, verified source, or trusted source")
    );
    assert_eq!(
        promotion_event
            .details
            .get("fallback_layer")
            .map(String::as_str),
        Some("episode")
    );
    assert_eq!(
        promotion_event
            .details
            .get("route_basis")
            .map(String::as_str),
        Some("insufficient_evidence")
    );
}

#[test]
fn ingest_verified_fact_promotes_belief_and_audits_route() {
    let (mut controller, sink) = controller(ts(1_700_000_000));
    let mut verified = candidate(
        "user",
        "location",
        "Berlin",
        "The verified profile says the user currently lives in Berlin.",
    );
    verified
        .metadata
        .insert("verified_source".to_string(), "true".to_string());

    let memory_id = controller
        .ingest(verified)
        .expect("ingest succeeds")
        .expect("durable memory stored");

    let hits = controller
        .retrieve(RetrievalQuery {
            query_text: "what is the user's current location".to_string(),
            intent: QueryIntent::CurrentFact,
            entity: Some("user".to_string()),
            slot: Some("location".to_string()),
            scope: None,
            top_k: 3,
            as_of: None,
            include_expired: false,
        })
        .expect("retrieval succeeds");

    assert!(controller.store().memories().len() >= 2);
    assert_eq!(controller.store().beliefs().len(), 1);
    assert_eq!(hits.first().map(|hit| hit.from_belief), Some(true));
    assert_eq!(
        hits.first().and_then(|hit| hit.value.as_deref()),
        Some("Berlin")
    );
    assert!(!memory_id.is_empty());

    let events = sink.events();
    let actions: Vec<_> = events.iter().map(|event| event.action.clone()).collect();
    assert_eq!(
        actions,
        vec![
            "classification".to_string(),
            "promotion".to_string(),
            "episode_stored".to_string(),
            "memory_stored".to_string(),
            "belief_updated".to_string(),
            "retrieval".to_string(),
        ]
    );

    let promotion_event = events
        .into_iter()
        .find(|event| event.action == "promotion")
        .expect("promotion audit event present");
    assert_eq!(
        promotion_event
            .details
            .get("target_layer")
            .map(String::as_str),
        Some("belief")
    );
    assert_eq!(
        promotion_event
            .details
            .get("route_basis")
            .map(String::as_str),
        Some("verified_source")
    );
    assert_eq!(
        promotion_event
            .details
            .get("verified_source")
            .map(String::as_str),
        Some("true")
    );

    let retrieval_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "retrieval")
        .expect("retrieval audit event present");
    assert_eq!(
        retrieval_event
            .details
            .get("touched_memories")
            .map(String::as_str),
        Some("2")
    );
}

#[test]
fn retrieval_touches_returned_memories_and_persists_access_metadata() {
    let stored_at = ts(1_700_000_000);
    let now = ts(1_700_000_100);
    let (mut controller, sink) = controller(now);
    let memory = durable(
        "user",
        "favorite_editor",
        "vim",
        "The user prefers vim for editing",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        stored_at,
    );

    let memory_id = apply_durable(&mut controller, &memory, None);
    assert_eq!(controller.store().memories().len(), 1);

    controller
        .retrieve(RetrievalQuery {
            query_text: "what editor does the user prefer".to_string(),
            intent: QueryIntent::PreferenceLookup,
            entity: Some("user".to_string()),
            slot: None,
            scope: None,
            top_k: 1,
            as_of: None,
            include_expired: false,
        })
        .expect("retrieval succeeds");

    let stored = controller
        .store()
        .memories()
        .iter()
        .find(|candidate| candidate.memory_id == memory_id)
        .expect("stored memory present")
        .clone();
    let latest = controller
        .store_mut()
        .get_memory(&memory_id)
        .expect("lookup succeeds")
        .expect("touched memory present");
    assert_eq!(controller.store().memories().len(), 1);
    assert_eq!(stored.stored_at, stored_at);
    assert_eq!(stored.version_timestamp(), stored_at);
    assert_eq!(latest.stored_at, stored_at);
    assert_eq!(latest.version_timestamp(), now);
    assert_eq!(
        latest.metadata.get("retrieval_count").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        latest.metadata.get("last_accessed_at").map(String::as_str),
        Some(now.to_rfc3339().as_str())
    );

    let retrieval_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "retrieval")
        .expect("retrieval audit event present");
    assert_eq!(
        retrieval_event
            .details
            .get("touched_memories")
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        retrieval_event
            .details
            .get("touched_memory_ids")
            .map(String::as_str),
        Some(memory_id.as_str())
    );
    assert_eq!(
        retrieval_event
            .details
            .get("touch_persistence")
            .map(String::as_str),
        Some("enabled")
    );
}

#[test]
fn retrieval_can_skip_durable_touch_writes_when_touch_persistence_is_disabled() {
    let stored_at = ts(1_700_000_000);
    let now = ts(1_700_000_100);
    let policy = PolicySet::default().with_persist_retrieval_touches(false);
    let (mut controller, sink) = controller_with_policy(now, policy);
    let memory = durable(
        "user",
        "favorite_editor",
        "vim",
        "The user prefers vim for editing",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        stored_at,
    );

    let memory_id = apply_durable(&mut controller, &memory, None);
    let hits = controller
        .retrieve(RetrievalQuery {
            query_text: "what editor does the user prefer".to_string(),
            intent: QueryIntent::PreferenceLookup,
            entity: Some("user".to_string()),
            slot: None,
            scope: None,
            top_k: 1,
            as_of: None,
            include_expired: false,
        })
        .expect("retrieval succeeds");

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].value.as_deref(), Some("vim"));

    let latest = controller
        .store_mut()
        .get_memory(&memory_id)
        .expect("lookup succeeds")
        .expect("memory exists");
    assert_eq!(latest.stored_at, stored_at);
    assert_eq!(latest.version_timestamp(), stored_at);
    assert_eq!(latest.retrieval_count(), 0);
    assert_eq!(latest.last_accessed_at(), None);

    let retrieval_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "retrieval")
        .expect("retrieval audit event present");
    assert_eq!(
        retrieval_event
            .details
            .get("touch_persistence")
            .map(String::as_str),
        Some("disabled")
    );
    assert!(!retrieval_event.details.contains_key("touched_memories"));
}

#[test]
fn outcome_feedback_updates_generic_memory_metadata() {
    let stored_at = ts(1_700_000_000);
    let now = ts(1_700_000_120);
    let (mut controller, sink) = controller(now);
    let memory = durable(
        "user",
        "favorite_editor",
        "vim",
        "The user prefers vim for editing",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        stored_at,
    );
    let memory_id = apply_durable(&mut controller, &memory, None);

    controller
        .record_outcome_feedback(OutcomeFeedback {
            memory_id: Some(memory_id.clone()),
            belief_id: None,
            workflow_key: None,
            outcome: OutcomeFeedbackKind::Positive,
            observed_at: now,
            metadata: std::collections::BTreeMap::from([(
                "source".to_string(),
                "task_execution".to_string(),
            )]),
        })
        .expect("feedback succeeds");

    let latest = controller
        .store()
        .memories()
        .iter()
        .rev()
        .find(|stored| stored.memory_id == memory_id)
        .expect("feedback memory present");
    assert_eq!(latest.stored_at, stored_at);
    assert_eq!(latest.version_timestamp(), now);
    assert_eq!(
        latest
            .metadata
            .get("positive_outcome_count")
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        latest.metadata.get("last_outcome_at").map(String::as_str),
        Some(now.to_rfc3339().as_str())
    );

    let feedback_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "outcome_feedback_recorded")
        .expect("feedback audit event present");
    assert_eq!(
        feedback_event
            .details
            .get("target_memory_id")
            .map(String::as_str),
        Some(memory_id.as_str())
    );
    assert_eq!(
        feedback_event.details.get("outcome").map(String::as_str),
        Some("positive")
    );
}

#[test]
fn negative_feedback_can_cool_down_a_procedure() {
    let now = ts(1_700_000_140);
    let (mut controller, sink) = controller(now);
    let mut procedure = durable(
        "procedure",
        "repo_review",
        "repo_review",
        "Review the repo in a consistent order",
        MemoryType::Trace,
        SourceType::System,
        1.0,
        ts(1_700_000_000),
    );
    procedure.internal_layer = Some(memvid_core::agent_memory::enums::MemoryLayer::Procedure);
    procedure
        .metadata
        .insert("workflow_key".to_string(), "repo_review".to_string());
    procedure
        .metadata
        .insert("procedure_name".to_string(), "repo_review".to_string());
    procedure
        .metadata
        .insert("context_tags".to_string(), "review,repo_review".to_string());
    procedure
        .metadata
        .insert("success_count".to_string(), "1".to_string());
    procedure
        .metadata
        .insert("failure_count".to_string(), "2".to_string());
    procedure
        .metadata
        .insert("procedure_status".to_string(), "active".to_string());
    let procedure_id = apply_durable(&mut controller, &procedure, None);

    controller
        .record_outcome_feedback(OutcomeFeedback {
            memory_id: Some(procedure_id.clone()),
            belief_id: None,
            workflow_key: None,
            outcome: OutcomeFeedbackKind::Negative,
            observed_at: now,
            metadata: std::collections::BTreeMap::new(),
        })
        .expect("feedback succeeds");

    let latest = controller
        .store()
        .memories()
        .iter()
        .rev()
        .find(|stored| stored.memory_id == procedure_id)
        .expect("procedure feedback present");
    assert_eq!(
        latest.metadata.get("failure_count").map(String::as_str),
        Some("3")
    );
    assert_eq!(
        latest
            .metadata
            .get("negative_outcome_count")
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        latest.metadata.get("procedure_status").map(String::as_str),
        Some("cooling_down")
    );

    let actions: Vec<_> = sink
        .events()
        .into_iter()
        .map(|event| event.action)
        .collect();
    assert!(
        actions
            .iter()
            .any(|action| action == "outcome_feedback_recorded")
    );
    assert!(
        actions
            .iter()
            .any(|action| action == "procedure_status_changed")
    );
}

#[test]
fn outcome_feedback_updates_belief_by_id_and_improves_effective_confidence() {
    let now = ts(1_700_000_160);
    let (mut controller, sink) = controller(ts(1_700_000_000));
    let mut verified = candidate(
        "user",
        "location",
        "Berlin",
        "The verified profile says the user currently lives in Berlin.",
    );
    verified
        .metadata
        .insert("verified_source".to_string(), "true".to_string());
    controller.ingest(verified).expect("ingest succeeds");

    let belief_id = controller
        .store()
        .beliefs()
        .values()
        .next()
        .expect("belief exists")
        .belief_id
        .clone();

    controller
        .record_outcome_feedback(OutcomeFeedback {
            memory_id: None,
            belief_id: Some(belief_id.clone()),
            workflow_key: None,
            outcome: OutcomeFeedbackKind::Positive,
            observed_at: now,
            metadata: std::collections::BTreeMap::new(),
        })
        .expect("belief feedback succeeds");

    let updated_belief = controller
        .store()
        .beliefs()
        .values()
        .next()
        .expect("updated belief exists")
        .clone();
    assert_eq!(updated_belief.positive_outcome_count, 1);
    assert_eq!(updated_belief.negative_outcome_count, 0);
    assert_eq!(updated_belief.last_outcome_at, Some(now));
    assert!(updated_belief.effective_confidence(now) > updated_belief.confidence);

    let hits = controller
        .retrieve(RetrievalQuery {
            query_text: "what is the user's current location".to_string(),
            intent: QueryIntent::CurrentFact,
            entity: Some("user".to_string()),
            slot: Some("location".to_string()),
            scope: None,
            top_k: 1,
            as_of: None,
            include_expired: false,
        })
        .expect("retrieval succeeds");
    let direct = hits.first().expect("direct belief hit");
    assert_eq!(
        direct
            .metadata
            .get("positive_outcome_count")
            .map(String::as_str),
        Some("1")
    );
    assert!(
        direct
            .metadata
            .get("score_signal_evidence_strength")
            .and_then(|value| value.parse::<f32>().ok())
            .expect("effective evidence strength")
            > updated_belief.confidence
    );

    let feedback_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "outcome_feedback_recorded")
        .expect("feedback audit event present");
    assert_eq!(
        feedback_event.belief_id.as_deref(),
        Some(belief_id.as_str())
    );
    assert_eq!(feedback_event.memory_id, None);
    assert_eq!(
        feedback_event
            .details
            .get("target_belief_id")
            .map(String::as_str),
        Some(belief_id.as_str())
    );
    assert_eq!(
        feedback_event
            .details
            .get("target_layer")
            .map(String::as_str),
        Some("belief")
    );
}

#[derive(Debug, Clone, Default)]
struct CountingTouchStore {
    memories: HashMap<String, DurableMemory>,
    hits: Vec<RetrievalHit>,
    batch_calls: usize,
    single_calls: usize,
    commit_like_operations: usize,
    last_batch_ids: Vec<String>,
    persists_access_touches: bool,
}

impl CountingTouchStore {
    fn with_hits(memories: Vec<DurableMemory>, hits: Vec<RetrievalHit>) -> Self {
        Self {
            memories: memories
                .into_iter()
                .map(|memory| (memory.memory_id.clone(), memory))
                .collect(),
            hits,
            persists_access_touches: true,
            ..Self::default()
        }
    }

    fn with_access_touch_persistence(mut self, enabled: bool) -> Self {
        self.persists_access_touches = enabled;
        self
    }
}

impl MemoryStore for CountingTouchStore {
    fn persists_access_touches(&self) -> bool {
        self.persists_access_touches
    }

    fn put_trace(
        &mut self,
        _raw_text: &str,
        _metadata: BTreeMap<String, String>,
    ) -> Result<String> {
        Ok("trace".to_string())
    }

    fn put_memory(&mut self, memory: &DurableMemory) -> Result<String> {
        self.memories
            .insert(memory.memory_id.clone(), memory.clone());
        Ok(memory.memory_id.clone())
    }

    fn touch_memory_access(&mut self, _memory_id: &str, _accessed_at: DateTime<Utc>) -> Result<()> {
        self.single_calls += 1;
        Ok(())
    }

    fn touch_memory_accesses(&mut self, touches: &[(String, DateTime<Utc>)]) -> Result<()> {
        self.batch_calls += 1;
        self.commit_like_operations += usize::from(!touches.is_empty());
        self.last_batch_ids = touches
            .iter()
            .map(|(memory_id, _)| memory_id.clone())
            .collect();
        Ok(())
    }

    fn update_belief(&mut self, _belief: &BeliefRecord) -> Result<()> {
        Ok(())
    }

    fn get_active_belief(&mut self, _entity: &str, _slot: &str) -> Result<Option<BeliefRecord>> {
        Ok(None)
    }

    fn get_current_belief(&mut self, _entity: &str, _slot: &str) -> Result<Option<BeliefRecord>> {
        Ok(None)
    }

    fn get_belief_by_id(&mut self, _belief_id: &str) -> Result<Option<BeliefRecord>> {
        Ok(None)
    }

    fn get_memory(&mut self, memory_id: &str) -> Result<Option<DurableMemory>> {
        Ok(self.memories.get(memory_id).cloned())
    }

    fn search(&mut self, _query: &RetrievalQuery) -> Result<Vec<RetrievalHit>> {
        Ok(self.hits.clone())
    }

    fn list_memory_versions_by_layer(
        &mut self,
        layer: memvid_core::agent_memory::enums::MemoryLayer,
    ) -> Result<Vec<DurableMemory>> {
        Ok(self
            .memories
            .values()
            .filter(|memory| memory.memory_layer() == layer)
            .cloned()
            .collect())
    }

    fn list_memories_by_layer(
        &mut self,
        layer: memvid_core::agent_memory::enums::MemoryLayer,
    ) -> Result<Vec<DurableMemory>> {
        self.list_memory_versions_by_layer(layer)
    }

    fn list_memories_for_belief(
        &mut self,
        _entity: &str,
        _slot: &str,
    ) -> Result<Vec<DurableMemory>> {
        Ok(Vec::new())
    }

    fn expire_memory(&mut self, _memory_id: &str) -> Result<()> {
        Ok(())
    }
}

#[test]
fn retrieval_batches_access_touches_once_per_operation() {
    let now = ts(1_700_000_180);
    let sink = InMemoryAuditSink::default();
    let clock = Arc::new(FixedClock::new(now));
    let policy = PolicySet::default();
    let first = durable(
        "user",
        "favorite_editor",
        "vim",
        "The user prefers vim for editing",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    let second = durable(
        "user",
        "favorite_shell",
        "fish",
        "The user prefers fish for shell work",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_010),
    );
    let hits = vec![
        RetrievalHit {
            memory_id: Some(first.memory_id.clone()),
            belief_id: None,
            entity: Some(first.entity.clone()),
            slot: Some(first.slot.clone()),
            value: Some(first.value.clone()),
            text: first.raw_text.clone(),
            memory_layer: Some(first.memory_layer()),
            memory_type: Some(first.memory_type),
            score: 0.9,
            timestamp: first.event_timestamp(),
            scope: Some(first.scope),
            source: Some(first.source.source_type),
            from_belief: false,
            expired: false,
            metadata: BTreeMap::new(),
        },
        RetrievalHit {
            memory_id: Some(second.memory_id.clone()),
            belief_id: None,
            entity: Some(second.entity.clone()),
            slot: Some(second.slot.clone()),
            value: Some(second.value.clone()),
            text: second.raw_text.clone(),
            memory_layer: Some(second.memory_layer()),
            memory_type: Some(second.memory_type),
            score: 0.8,
            timestamp: second.event_timestamp(),
            scope: Some(second.scope),
            source: Some(second.source.source_type),
            from_belief: false,
            expired: false,
            metadata: BTreeMap::new(),
        },
        RetrievalHit {
            memory_id: Some("trace:1".to_string()),
            belief_id: None,
            entity: None,
            slot: None,
            value: None,
            text: "trace".to_string(),
            memory_layer: Some(memvid_core::agent_memory::enums::MemoryLayer::Trace),
            memory_type: Some(MemoryType::Trace),
            score: 0.2,
            timestamp: now,
            scope: None,
            source: None,
            from_belief: false,
            expired: false,
            metadata: BTreeMap::new(),
        },
    ];
    let store = CountingTouchStore::with_hits(vec![first.clone(), second.clone()], hits);
    let mut controller = MemoryController::new(
        store,
        clock.clone(),
        AuditLogger::new(clock.clone(), Arc::new(sink)),
        MemoryClassifier,
        MemoryPromoter::new(policy.clone()),
        BeliefUpdater,
        MemoryRetriever::new(Ranker, RetentionManager::new(policy)),
    );

    controller
        .retrieve(RetrievalQuery {
            query_text: "preferred tools".to_string(),
            intent: QueryIntent::SemanticBackground,
            entity: None,
            slot: None,
            scope: None,
            top_k: 5,
            as_of: None,
            include_expired: false,
        })
        .expect("retrieval succeeds");

    assert_eq!(controller.store().batch_calls, 1);
    assert_eq!(controller.store().single_calls, 0);
    assert_eq!(controller.store().commit_like_operations, 1);
    assert_eq!(
        controller.store().last_batch_ids,
        vec![first.memory_id, second.memory_id]
    );
}

#[test]
fn retrieval_audit_reflects_store_level_touch_persistence_disablement() {
    let now = ts(1_700_000_180);
    let sink = InMemoryAuditSink::default();
    let clock = Arc::new(FixedClock::new(now));
    let policy = PolicySet::default();
    let memory = durable(
        "user",
        "favorite_editor",
        "vim",
        "The user prefers vim for editing",
        MemoryType::Preference,
        SourceType::Chat,
        0.75,
        ts(1_700_000_000),
    );
    let hits = vec![RetrievalHit {
        memory_id: Some(memory.memory_id.clone()),
        belief_id: None,
        entity: Some(memory.entity.clone()),
        slot: Some(memory.slot.clone()),
        value: Some(memory.value.clone()),
        text: memory.raw_text.clone(),
        memory_layer: Some(memory.memory_layer()),
        memory_type: Some(memory.memory_type),
        score: 0.9,
        timestamp: memory.event_timestamp(),
        scope: Some(memory.scope),
        source: Some(memory.source.source_type),
        from_belief: false,
        expired: false,
        metadata: BTreeMap::new(),
    }];
    let store = CountingTouchStore::with_hits(vec![memory.clone()], hits)
        .with_access_touch_persistence(false);
    let mut controller = MemoryController::new(
        store,
        clock.clone(),
        AuditLogger::new(clock.clone(), Arc::new(sink.clone())),
        MemoryClassifier,
        MemoryPromoter::new(policy.clone()),
        BeliefUpdater,
        MemoryRetriever::new(Ranker, RetentionManager::new(policy)),
    );

    controller
        .retrieve(RetrievalQuery {
            query_text: "preferred editor".to_string(),
            intent: QueryIntent::PreferenceLookup,
            entity: Some("user".to_string()),
            slot: None,
            scope: None,
            top_k: 1,
            as_of: None,
            include_expired: false,
        })
        .expect("retrieval succeeds");

    assert_eq!(controller.store().batch_calls, 0);
    assert_eq!(controller.store().single_calls, 0);
    assert_eq!(controller.store().commit_like_operations, 0);
    assert!(controller.store().last_batch_ids.is_empty());

    let retrieval_event = sink
        .events()
        .into_iter()
        .find(|event| event.action == "retrieval")
        .expect("retrieval audit event present");
    assert_eq!(
        retrieval_event
            .details
            .get("touch_persistence")
            .map(String::as_str),
        Some("disabled")
    );
    assert!(!retrieval_event.details.contains_key("touched_memories"));
    assert!(!retrieval_event.details.contains_key("touched_memory_ids"));
}
