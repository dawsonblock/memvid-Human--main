use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::Memvid;
use crate::agent_memory::clock::{Clock, SystemClock};
use crate::agent_memory::enums::{
    BeliefStatus, MemoryLayer, MemoryType, QueryIntent, Scope, SourceType,
};
use crate::agent_memory::errors::{AgentMemoryError, Result};
use crate::agent_memory::schemas::{BeliefRecord, DurableMemory, RetrievalHit, RetrievalQuery};
use crate::types::{AclEnforcementMode, MemoryCardBuilder, MemoryKind, PutOptions, SearchRequest};

const TRACK_TRACE: &str = "agent_memory_trace";
const TRACK_MEMORY: &str = "agent_memory_memory";
const TRACK_BELIEF: &str = "agent_memory_belief";
const TRACK_SYSTEM: &str = "agent_memory_system";
const BELIEF_PREFIX: &str = "__agent_memory_belief__";
const EXPIRY_PREFIX: &str = "__agent_memory_expiry__";
const ACCESS_PREFIX: &str = "__agent_memory_access__";

type AccessTouch = (DateTime<Utc>, u32);

/// Narrow governed-memory store abstraction.
pub trait MemoryStore {
    fn put_trace(&mut self, raw_text: &str, metadata: BTreeMap<String, String>) -> Result<String>;
    fn put_memory(&mut self, memory: &DurableMemory) -> Result<String>;
    fn persists_access_touches(&self) -> bool {
        true
    }
    fn touch_memory_access(&mut self, memory_id: &str, accessed_at: DateTime<Utc>) -> Result<()>;
    fn touch_memory_accesses(&mut self, touches: &[(String, DateTime<Utc>)]) -> Result<()> {
        for (memory_id, accessed_at) in touches {
            self.touch_memory_access(memory_id, accessed_at.to_owned())?;
        }
        Ok(())
    }
    fn update_belief(&mut self, belief: &BeliefRecord) -> Result<()>;
    fn get_active_belief(&mut self, entity: &str, slot: &str) -> Result<Option<BeliefRecord>>;
    fn get_current_belief(&mut self, entity: &str, slot: &str) -> Result<Option<BeliefRecord>>;
    fn get_belief_by_id(&mut self, belief_id: &str) -> Result<Option<BeliefRecord>>;
    fn get_memory(&mut self, memory_id: &str) -> Result<Option<DurableMemory>>;
    fn search(&mut self, query: &RetrievalQuery) -> Result<Vec<RetrievalHit>>;
    fn list_memory_versions_by_layer(&mut self, layer: MemoryLayer) -> Result<Vec<DurableMemory>>;
    fn list_memories_by_layer(&mut self, layer: MemoryLayer) -> Result<Vec<DurableMemory>>;
    fn list_memories_for_belief(&mut self, entity: &str, slot: &str) -> Result<Vec<DurableMemory>>;
    fn expire_memory(&mut self, memory_id: &str) -> Result<()>;
}

fn scope_string(scope: Scope) -> &'static str {
    match scope {
        Scope::Private => "private",
        Scope::Task => "task",
        Scope::Project => "project",
        Scope::Shared => "shared",
    }
}

fn parse_scope(value: Option<&String>) -> Scope {
    match value.map(std::string::String::as_str) {
        Some("task") => Scope::Task,
        Some("project") => Scope::Project,
        Some("shared") => Scope::Shared,
        _ => Scope::Private,
    }
}

fn parse_memory_type(value: Option<&String>) -> MemoryType {
    match value.map(std::string::String::as_str) {
        Some("episode") => MemoryType::Episode,
        Some("fact") => MemoryType::Fact,
        Some("preference") => MemoryType::Preference,
        Some("goalstate" | "goal_state") => MemoryType::GoalState,
        _ => MemoryType::Trace,
    }
}

fn parse_memory_layer(value: Option<&String>, memory_type: MemoryType) -> MemoryLayer {
    value
        .and_then(|text| MemoryLayer::from_str(text))
        .unwrap_or_else(|| memory_type.memory_layer())
}

fn parse_source_type(value: Option<&String>) -> SourceType {
    match value.map(std::string::String::as_str) {
        Some("file") => SourceType::File,
        Some("tool") => SourceType::Tool,
        Some("system") => SourceType::System,
        Some("external") => SourceType::External,
        _ => SourceType::Chat,
    }
}

fn memory_kind(memory: &DurableMemory) -> MemoryKind {
    match memory.memory_layer() {
        MemoryLayer::Trace | MemoryLayer::Procedure => MemoryKind::Other,
        MemoryLayer::Episode => MemoryKind::Event,
        MemoryLayer::Belief => MemoryKind::Fact,
        MemoryLayer::SelfModel => MemoryKind::Preference,
        MemoryLayer::GoalState => MemoryKind::Goal,
    }
}

fn timestamp_to_datetime(timestamp: i64) -> Result<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(timestamp, 0).ok_or_else(|| AgentMemoryError::Store {
        reason: format!("invalid unix timestamp: {timestamp}"),
    })
}

fn parse_datetime(value: Option<&String>) -> Result<Option<DateTime<Utc>>> {
    match value {
        Some(text) => Ok(Some(
            DateTime::parse_from_rfc3339(text)
                .map_err(|err| AgentMemoryError::Store {
                    reason: format!("invalid timestamp '{text}': {err}"),
                })?
                .with_timezone(&Utc),
        )),
        None => Ok(None),
    }
}

fn memory_metadata(memory: &DurableMemory) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::from([
        ("agent_memory_id".to_string(), memory.memory_id.clone()),
        (
            "agent_candidate_id".to_string(),
            memory.candidate_id.clone(),
        ),
        ("agent_entity".to_string(), memory.entity.clone()),
        ("agent_slot".to_string(), memory.slot.clone()),
        ("agent_value".to_string(), memory.value.clone()),
        (
            "agent_memory_type".to_string(),
            format!("{:?}", memory.memory_type).to_lowercase(),
        ),
        (
            "agent_memory_layer".to_string(),
            memory.memory_layer().as_str().to_string(),
        ),
        (
            "agent_source_id".to_string(),
            memory.source.source_id.clone(),
        ),
        (
            "agent_confidence".to_string(),
            memory.confidence.to_string(),
        ),
        ("agent_salience".to_string(), memory.salience.to_string()),
        (
            "agent_scope".to_string(),
            scope_string(memory.scope).to_string(),
        ),
        (
            "agent_source_type".to_string(),
            format!("{:?}", memory.source.source_type).to_lowercase(),
        ),
        (
            "agent_source_weight".to_string(),
            memory.source.trust_weight.to_string(),
        ),
        (
            "agent_source_label".to_string(),
            memory.source.source_label.clone().unwrap_or_default(),
        ),
        (
            "agent_observed_at".to_string(),
            memory.event_timestamp().to_rfc3339(),
        ),
        ("agent_stored_at".to_string(), memory.stored_at.to_rfc3339()),
        (
            "agent_updated_at".to_string(),
            memory.version_timestamp().to_rfc3339(),
        ),
        (
            "agent_is_retraction".to_string(),
            memory.is_retraction.to_string(),
        ),
    ]);
    if let Some(ttl) = memory.ttl {
        metadata.insert("agent_ttl".to_string(), ttl.to_string());
    }
    if let Some(event_at) = memory.event_at {
        metadata.insert("agent_event_at".to_string(), event_at.to_rfc3339());
    }
    if let Some(valid_from) = memory.valid_from {
        metadata.insert("agent_valid_from".to_string(), valid_from.to_rfc3339());
    }
    if let Some(valid_to) = memory.valid_to {
        metadata.insert("agent_valid_to".to_string(), valid_to.to_rfc3339());
    }
    if !memory.tags.is_empty() {
        metadata.insert("agent_tags".to_string(), memory.tags.join(","));
    }
    for (key, value) in &memory.metadata {
        metadata.insert(format!("agent_meta_{key}"), value.clone());
    }
    metadata
}

fn retrieval_metadata(memory: &DurableMemory) -> BTreeMap<String, String> {
    let mut metadata = memory.metadata.clone();
    metadata.insert(
        "memory_layer".to_string(),
        memory.memory_layer().as_str().to_string(),
    );
    metadata.insert("confidence".to_string(), memory.confidence.to_string());
    metadata.insert("salience".to_string(), memory.salience.to_string());
    metadata.insert("source_id".to_string(), memory.source.source_id.clone());
    metadata.insert(
        "source_type".to_string(),
        format!("{:?}", memory.source.source_type).to_lowercase(),
    );
    metadata.insert(
        "source_weight".to_string(),
        memory.source.trust_weight.to_string(),
    );
    metadata.insert("stored_at".to_string(), memory.stored_at.to_rfc3339());
    metadata.insert(
        "updated_at".to_string(),
        memory.version_timestamp().to_rfc3339(),
    );
    metadata.insert(
        "event_at".to_string(),
        memory
            .event_at
            .unwrap_or(memory.event_timestamp())
            .to_rfc3339(),
    );
    if let Some(valid_from) = memory.valid_from {
        metadata.insert("valid_from".to_string(), valid_from.to_rfc3339());
    }
    if let Some(valid_to) = memory.valid_to {
        metadata.insert("valid_to".to_string(), valid_to.to_rfc3339());
    }
    metadata
}

fn memory_is_newer(candidate: &DurableMemory, existing: &DurableMemory) -> bool {
    candidate.version_timestamp() > existing.version_timestamp()
        || (candidate.version_timestamp() == existing.version_timestamp()
            && (candidate.event_timestamp() > existing.event_timestamp()
                || (candidate.event_timestamp() == existing.event_timestamp()
                    && candidate.memory_id > existing.memory_id)))
}

fn latest_memories_by_id(memories: impl IntoIterator<Item = DurableMemory>) -> Vec<DurableMemory> {
    let mut latest = HashMap::new();
    for memory in memories {
        match latest.get(&memory.memory_id) {
            Some(existing) if !memory_is_newer(&memory, existing) => {}
            _ => {
                latest.insert(memory.memory_id.clone(), memory);
            }
        }
    }
    latest.into_values().collect()
}

fn hit_identity(hit: &RetrievalHit) -> Option<String> {
    hit.memory_id
        .as_ref()
        .map(|memory_id| format!("memory:{memory_id}"))
        .or_else(|| {
            hit.belief_id
                .as_ref()
                .map(|belief_id| format!("belief:{belief_id}"))
        })
}

fn dedup_search_hits(hits: Vec<RetrievalHit>) -> Vec<RetrievalHit> {
    let mut deduped: HashMap<String, RetrievalHit> = HashMap::new();
    for hit in hits {
        let Some(identity) = hit_identity(&hit) else {
            continue;
        };
        match deduped.get(&identity) {
            Some(existing)
                if existing.score.total_cmp(&hit.score).is_gt()
                    || (existing.score.total_cmp(&hit.score).is_eq()
                        && existing.timestamp >= hit.timestamp) => {}
            _ => {
                deduped.insert(identity, hit);
            }
        }
    }

    let mut hits: Vec<_> = deduped.into_values().collect();
    hits.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| right.timestamp.cmp(&left.timestamp))
    });
    hits
}

fn aggregate_batch_touches(
    touches: &[(String, DateTime<Utc>)],
) -> Vec<(String, DateTime<Utc>, u32)> {
    let mut aggregated: Vec<(String, DateTime<Utc>, u32)> = Vec::new();
    let mut index_by_memory_id: HashMap<&str, usize> = HashMap::new();

    for (memory_id, accessed_at) in touches {
        if let Some(index) = index_by_memory_id.get(memory_id.as_str()).copied() {
            let (_, latest_accessed_at, occurrences) = &mut aggregated[index];
            *latest_accessed_at = (*latest_accessed_at).max(*accessed_at);
            *occurrences = occurrences.saturating_add(1);
        } else {
            index_by_memory_id.insert(memory_id.as_str(), aggregated.len());
            aggregated.push((memory_id.clone(), *accessed_at, 1));
        }
    }

    aggregated
}

fn belief_entity(entity: &str) -> String {
    format!("{BELIEF_PREFIX}:{entity}")
}

fn expiry_entity() -> &'static str {
    EXPIRY_PREFIX
}

fn access_entity() -> &'static str {
    ACCESS_PREFIX
}

fn is_reserved_system_entity(entity: &str) -> bool {
    entity == expiry_entity() || entity == access_entity()
}

fn memory_as_of_anchor(query: &RetrievalQuery, memory: &DurableMemory) -> DateTime<Utc> {
    match query.intent {
        QueryIntent::HistoricalFact | QueryIntent::EpisodicRecall => memory.event_timestamp(),
        _ => memory.stored_at,
    }
}

fn simple_score(haystack: &str, query: &str) -> f32 {
    let haystack_lower = haystack.to_lowercase();
    let normalized_query = query.to_lowercase();
    let tokens: Vec<_> = normalized_query
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() {
        return 0.0;
    }
    let matches = tokens
        .iter()
        .filter(|token| haystack_lower.contains(**token))
        .count();
    matches as f32 / tokens.len() as f32
}

/// In-memory test store.
#[derive(Debug, Default, Clone)]
pub struct InMemoryMemoryStore {
    traces: Vec<(String, String, BTreeMap<String, String>)>,
    memories: Vec<DurableMemory>,
    beliefs: HashMap<(String, String), BeliefRecord>,
    expired: HashSet<String>,
    access_touches: HashMap<String, AccessTouch>,
}

impl InMemoryMemoryStore {
    #[must_use]
    pub fn memories(&self) -> &[DurableMemory] {
        &self.memories
    }

    #[must_use]
    pub fn traces(&self) -> &[(String, String, BTreeMap<String, String>)] {
        &self.traces
    }

    #[must_use]
    pub fn beliefs(&self) -> &HashMap<(String, String), BeliefRecord> {
        &self.beliefs
    }

    fn latest_stored_memory(&self, memory_id: &str) -> Option<DurableMemory> {
        latest_memories_by_id(
            self.memories
                .iter()
                .filter(|memory| memory.memory_id == memory_id)
                .cloned(),
        )
        .into_iter()
        .next()
    }

    fn apply_access_touch(&self, mut memory: DurableMemory) -> DurableMemory {
        if let Some((accessed_at, retrieval_count)) = self.access_touches.get(&memory.memory_id)
            && memory
                .last_accessed_at()
                .is_none_or(|existing| *accessed_at > existing)
        {
            memory
                .metadata
                .insert("retrieval_count".to_string(), retrieval_count.to_string());
            memory
                .metadata
                .insert("last_accessed_at".to_string(), accessed_at.to_rfc3339());
            memory.updated_at = Some(memory.version_timestamp().max(accessed_at.to_owned()));
        }
        memory
    }
}

impl MemoryStore for InMemoryMemoryStore {
    fn put_trace(&mut self, raw_text: &str, metadata: BTreeMap<String, String>) -> Result<String> {
        let trace_id = Uuid::new_v4().to_string();
        self.traces
            .push((trace_id.clone(), raw_text.to_string(), metadata));
        Ok(trace_id)
    }

    fn put_memory(&mut self, memory: &DurableMemory) -> Result<String> {
        self.memories.push(memory.clone());
        Ok(memory.memory_id.clone())
    }

    fn touch_memory_access(&mut self, memory_id: &str, accessed_at: DateTime<Utc>) -> Result<()> {
        self.touch_memory_accesses(&[(memory_id.to_string(), accessed_at)])
    }

    fn touch_memory_accesses(&mut self, touches: &[(String, DateTime<Utc>)]) -> Result<()> {
        for (memory_id, accessed_at, occurrences) in aggregate_batch_touches(touches) {
            let Some(memory) = self.get_memory(&memory_id)? else {
                continue;
            };
            let mut touched = memory;
            touched.metadata.insert(
                "retrieval_count".to_string(),
                touched
                    .retrieval_count()
                    .saturating_add(occurrences)
                    .to_string(),
            );
            touched
                .metadata
                .insert("last_accessed_at".to_string(), accessed_at.to_rfc3339());
            touched.updated_at = Some(touched.version_timestamp().max(accessed_at));
            self.access_touches
                .insert(memory_id, (accessed_at, touched.retrieval_count()));
        }

        Ok(())
    }

    fn update_belief(&mut self, belief: &BeliefRecord) -> Result<()> {
        self.beliefs
            .insert((belief.entity.clone(), belief.slot.clone()), belief.clone());
        Ok(())
    }

    fn get_active_belief(&mut self, entity: &str, slot: &str) -> Result<Option<BeliefRecord>> {
        Ok(self
            .beliefs
            .get(&(entity.to_string(), slot.to_string()))
            .cloned()
            .filter(|belief| belief.status == BeliefStatus::Active))
    }

    fn get_current_belief(&mut self, entity: &str, slot: &str) -> Result<Option<BeliefRecord>> {
        Ok(self
            .beliefs
            .get(&(entity.to_string(), slot.to_string()))
            .cloned())
    }

    fn get_belief_by_id(&mut self, belief_id: &str) -> Result<Option<BeliefRecord>> {
        Ok(self
            .beliefs
            .values()
            .find(|belief| belief.belief_id == belief_id)
            .cloned())
    }

    fn get_memory(&mut self, memory_id: &str) -> Result<Option<DurableMemory>> {
        Ok(self
            .latest_stored_memory(memory_id)
            .map(|memory| self.apply_access_touch(memory)))
    }

    fn search(&mut self, query: &RetrievalQuery) -> Result<Vec<RetrievalHit>> {
        let mut hits = Vec::new();
        for memory in latest_memories_by_id(self.memories.clone())
            .into_iter()
            .map(|memory| self.apply_access_touch(memory))
        {
            if let Some(scope) = query.scope
                && memory.scope != scope
            {
                continue;
            }
            if self.expired.contains(&memory.memory_id) && !query.include_expired {
                continue;
            }
            if let Some(as_of) = query.as_of
                && memory_as_of_anchor(query, &memory) > as_of
            {
                continue;
            }
            let text = format!(
                "{} {} {} {}",
                memory.entity, memory.slot, memory.value, memory.raw_text
            );
            let score = simple_score(&text, &query.query_text);
            if score == 0.0 {
                continue;
            }
            hits.push(RetrievalHit {
                memory_id: Some(memory.memory_id.clone()),
                belief_id: None,
                entity: Some(memory.entity.clone()),
                slot: Some(memory.slot.clone()),
                value: Some(memory.value.clone()),
                text: memory.raw_text.clone(),
                memory_layer: Some(memory.memory_layer()),
                memory_type: Some(memory.memory_type),
                score,
                timestamp: memory.event_timestamp(),
                scope: Some(memory.scope),
                source: Some(memory.source.source_type),
                from_belief: false,
                expired: self.expired.contains(&memory.memory_id),
                metadata: retrieval_metadata(&memory),
            });
        }
        for (trace_id, raw_text, metadata) in &self.traces {
            let score = simple_score(
                &format!(
                    "{} {} {} {} {} {}",
                    raw_text,
                    metadata.get("workflow_key").map_or("", String::as_str),
                    metadata.get("previous_status").map_or("", String::as_str),
                    metadata.get("next_status").map_or("", String::as_str),
                    metadata.get("transition_reason").map_or("", String::as_str),
                    metadata.get("source").map_or("", String::as_str),
                ),
                &query.query_text,
            );
            if score == 0.0 {
                continue;
            }

            let timestamp = metadata
                .get("occurred_at")
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);

            hits.push(RetrievalHit {
                memory_id: Some(trace_id.clone()),
                belief_id: None,
                entity: metadata.get("entity").cloned(),
                slot: metadata.get("slot").cloned(),
                value: metadata.get("value").cloned(),
                text: raw_text.clone(),
                memory_layer: Some(MemoryLayer::Trace),
                memory_type: Some(MemoryType::Trace),
                score,
                timestamp,
                scope: query.scope,
                source: metadata
                    .get("source_type")
                    .map(|value| parse_source_type(Some(value))),
                from_belief: false,
                expired: false,
                metadata: metadata.clone(),
            });
        }
        let mut hits = dedup_search_hits(hits);
        hits.truncate(query.top_k.saturating_mul(4));
        Ok(hits)
    }

    fn list_memories_by_layer(&mut self, layer: MemoryLayer) -> Result<Vec<DurableMemory>> {
        Ok(
            latest_memories_by_id(self.list_memory_versions_by_layer(layer)?)
                .into_iter()
                .map(|memory| self.apply_access_touch(memory))
                .collect(),
        )
    }

    fn list_memory_versions_by_layer(&mut self, layer: MemoryLayer) -> Result<Vec<DurableMemory>> {
        Ok(self
            .memories
            .iter()
            .filter(|memory| memory.memory_layer() == layer)
            .cloned()
            .collect())
    }

    fn list_memories_for_belief(&mut self, entity: &str, slot: &str) -> Result<Vec<DurableMemory>> {
        Ok(self
            .list_memories_by_layer(MemoryLayer::Belief)?
            .into_iter()
            .filter(|memory| memory.entity == entity && memory.slot == slot)
            .collect())
    }

    fn expire_memory(&mut self, memory_id: &str) -> Result<()> {
        self.expired.insert(memory_id.to_string());
        Ok(())
    }
}

/// Real adapter over memvid frame + memories APIs.
pub struct MemvidStore {
    memvid: Memvid,
    access_touch_cache: HashMap<String, Option<AccessTouch>>,
    persist_access_touches: bool,
    clock: Arc<dyn Clock>,
}

impl MemvidStore {
    #[must_use]
    pub fn new(memvid: Memvid) -> Self {
        Self::with_access_touch_persistence(memvid, true)
    }

    #[must_use]
    pub fn with_access_touch_persistence(memvid: Memvid, persist_access_touches: bool) -> Self {
        Self {
            memvid,
            access_touch_cache: HashMap::new(),
            persist_access_touches,
            clock: Arc::new(SystemClock),
        }
    }

    /// Override the clock used for store-level timestamps (trace ingest, expiry).
    /// Defaults to [`SystemClock`]. Useful for deterministic tests.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn set_access_touch_persistence(&mut self, enabled: bool) {
        self.persist_access_touches = enabled;
    }

    fn is_expired(&self, memory_id: &str) -> bool {
        self.memvid
            .memories()
            .get_cards(expiry_entity(), memory_id)
            .iter()
            .any(|card| !card.is_retracted())
    }

    fn cache_access_touch(&mut self, memory_id: String, touch: Option<AccessTouch>) {
        self.access_touch_cache.insert(memory_id, touch);
    }

    fn latest_access_touch(&mut self, memory_id: &str) -> Result<Option<AccessTouch>> {
        if let Some(touch) = self.access_touch_cache.get(memory_id) {
            return Ok(*touch);
        }

        let mut latest = None;
        for card in self.memvid.memories().get_cards(access_entity(), memory_id) {
            if card.is_retracted() {
                continue;
            }
            let frame = self.memvid.frame_by_id(card.source_frame_id)?;
            let Some(accessed_at) =
                parse_datetime(frame.extra_metadata.get("agent_last_accessed_at"))?
            else {
                continue;
            };
            let retrieval_count = frame
                .extra_metadata
                .get("agent_retrieval_count")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0);
            if latest
                .as_ref()
                .is_none_or(|(current_accessed_at, _)| accessed_at > *current_accessed_at)
            {
                latest = Some((accessed_at, retrieval_count));
            }
        }
        self.cache_access_touch(memory_id.to_string(), latest);
        Ok(latest)
    }

    fn apply_access_touch(&mut self, mut memory: DurableMemory) -> Result<DurableMemory> {
        if let Some((accessed_at, retrieval_count)) = self.latest_access_touch(&memory.memory_id)? {
            if memory
                .last_accessed_at()
                .is_none_or(|existing| accessed_at > existing)
            {
                memory
                    .metadata
                    .insert("retrieval_count".to_string(), retrieval_count.to_string());
                memory
                    .metadata
                    .insert("last_accessed_at".to_string(), accessed_at.to_rfc3339());
                memory.updated_at = Some(memory.version_timestamp().max(accessed_at));
            }
        }
        Ok(memory)
    }

    fn build_durable_from_frame(
        &mut self,
        frame_id: u64,
        extra_metadata: &BTreeMap<String, String>,
    ) -> Result<DurableMemory> {
        let raw_text = self.memvid.frame_text_by_id(frame_id)?;
        let mut metadata = BTreeMap::new();
        for (key, value) in extra_metadata {
            if let Some(stripped) = key.strip_prefix("agent_meta_") {
                metadata.insert(stripped.to_string(), value.clone());
            }
        }
        Ok(DurableMemory {
            memory_id: extra_metadata
                .get("agent_memory_id")
                .cloned()
                .ok_or_else(|| AgentMemoryError::Store {
                    reason: "missing agent_memory_id metadata".to_string(),
                })?,
            candidate_id: extra_metadata
                .get("agent_candidate_id")
                .cloned()
                .unwrap_or_default(),
            stored_at: parse_datetime(extra_metadata.get("agent_stored_at"))?
                .unwrap_or(timestamp_to_datetime(0)?),
            updated_at: Some(
                parse_datetime(extra_metadata.get("agent_updated_at"))?
                    .or(parse_datetime(extra_metadata.get("agent_stored_at"))?)
                    .unwrap_or(timestamp_to_datetime(0)?),
            ),
            entity: extra_metadata
                .get("agent_entity")
                .cloned()
                .unwrap_or_default(),
            slot: extra_metadata
                .get("agent_slot")
                .cloned()
                .unwrap_or_default(),
            value: extra_metadata
                .get("agent_value")
                .cloned()
                .unwrap_or_default(),
            raw_text,
            memory_type: parse_memory_type(extra_metadata.get("agent_memory_type")),
            confidence: extra_metadata
                .get("agent_confidence")
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(0.5),
            salience: extra_metadata
                .get("agent_salience")
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(0.5),
            scope: parse_scope(extra_metadata.get("agent_scope")),
            ttl: extra_metadata
                .get("agent_ttl")
                .and_then(|value| value.parse::<i64>().ok()),
            source: crate::agent_memory::schemas::Provenance {
                source_type: parse_source_type(extra_metadata.get("agent_source_type")),
                source_id: extra_metadata
                    .get("agent_source_id")
                    .cloned()
                    .unwrap_or_else(|| format!("frame:{frame_id}")),
                source_label: extra_metadata
                    .get("agent_source_label")
                    .cloned()
                    .filter(|value| !value.is_empty()),
                observed_by: extra_metadata
                    .get("agent_observed_by")
                    .cloned()
                    .filter(|value| !value.is_empty()),
                trust_weight: extra_metadata
                    .get("agent_source_weight")
                    .and_then(|value| value.parse::<f32>().ok())
                    .unwrap_or(0.5),
            },
            event_at: parse_datetime(extra_metadata.get("agent_event_at"))?,
            valid_from: parse_datetime(extra_metadata.get("agent_valid_from"))?,
            valid_to: parse_datetime(extra_metadata.get("agent_valid_to"))?,
            internal_layer: Some(parse_memory_layer(
                extra_metadata.get("agent_memory_layer"),
                parse_memory_type(extra_metadata.get("agent_memory_type")),
            )),
            tags: extra_metadata
                .get("agent_tags")
                .map(|value| value.split(',').map(ToString::to_string).collect())
                .unwrap_or_default(),
            metadata,
            is_retraction: extra_metadata
                .get("agent_is_retraction")
                .is_some_and(|value| value == "true"),
        })
    }

    fn search_memory_cards(&mut self, query: &RetrievalQuery) -> Result<Vec<RetrievalHit>> {
        let mut hits = Vec::new();
        let cards = self.memvid.memories().cards().to_vec();
        for card in &cards {
            if card.entity.starts_with(BELIEF_PREFIX) || is_reserved_system_entity(&card.entity) {
                continue;
            }
            let frame = self.memvid.frame_by_id(card.source_frame_id)?;
            let extra = frame.extra_metadata.clone();
            if !extra.contains_key("agent_memory_id") {
                continue;
            }
            let memory = self.build_durable_from_frame(card.source_frame_id, &extra)?;
            let memory = self.apply_access_touch(memory)?;
            if let Some(scope) = query.scope
                && memory.scope != scope
            {
                continue;
            }
            if let Some(as_of) = query.as_of
                && memory_as_of_anchor(query, &memory) > as_of
            {
                continue;
            }
            let score = simple_score(
                &format!(
                    "{} {} {} {}",
                    memory.entity, memory.slot, memory.value, memory.raw_text
                ),
                &query.query_text,
            );
            if score == 0.0 {
                continue;
            }
            hits.push(RetrievalHit {
                memory_id: Some(memory.memory_id.clone()),
                belief_id: None,
                entity: Some(memory.entity.clone()),
                slot: Some(memory.slot.clone()),
                value: Some(memory.value.clone()),
                text: memory.raw_text.clone(),
                memory_layer: Some(memory.memory_layer()),
                memory_type: Some(memory.memory_type),
                score,
                timestamp: memory.event_timestamp(),
                scope: Some(memory.scope),
                source: Some(memory.source.source_type),
                from_belief: false,
                expired: self.is_expired(&memory.memory_id),
                metadata: retrieval_metadata(&memory),
            });
        }
        Ok(hits)
    }

    fn frame_id_for_uri(&self, uri: &str) -> Result<u64> {
        Ok(self.memvid.frame_by_uri(uri)?.id)
    }

    fn access_touch_uri(memory_id: &str, accessed_at: DateTime<Utc>) -> String {
        format!(
            "mv2://agent-memory/access/{memory_id}/{}",
            accessed_at
                .timestamp_nanos_opt()
                .unwrap_or_else(|| accessed_at.timestamp_micros())
        )
    }
}

impl MemoryStore for MemvidStore {
    fn persists_access_touches(&self) -> bool {
        self.persist_access_touches
    }

    fn put_trace(&mut self, raw_text: &str, metadata: BTreeMap<String, String>) -> Result<String> {
        let trace_id = Uuid::new_v4().to_string();
        let uri = format!("mv2://agent-memory/trace/{trace_id}");
        let mut extra_metadata = metadata;
        extra_metadata.insert("agent_trace_id".to_string(), trace_id.clone());
        self.memvid.put_bytes_with_options(
            raw_text.as_bytes(),
            PutOptions {
                timestamp: Some(self.clock.now().timestamp()),
                track: Some(TRACK_TRACE.to_string()),
                kind: Some("agent_memory_trace".to_string()),
                uri: Some(uri.clone()),
                title: None,
                metadata: None,
                search_text: Some(raw_text.to_string()),
                tags: vec!["agent-memory".to_string(), "trace".to_string()],
                labels: Vec::new(),
                extra_metadata,
                ..PutOptions::default()
            },
        )?;
        self.memvid.commit()?;
        let frame_id = self.frame_id_for_uri(&uri)?;
        Ok(format!("trace:{frame_id}"))
    }

    fn put_memory(&mut self, memory: &DurableMemory) -> Result<String> {
        let uri = format!("mv2://agent-memory/memory/{}", memory.memory_id);
        self.memvid.put_bytes_with_options(
            memory.raw_text.as_bytes(),
            PutOptions {
                timestamp: Some(memory.version_timestamp().timestamp()),
                track: Some(TRACK_MEMORY.to_string()),
                kind: Some(format!("agent_memory_{}", memory.memory_layer().as_str())),
                uri: Some(uri.clone()),
                title: Some(format!("{}:{}", memory.entity, memory.slot)),
                metadata: None,
                search_text: Some(memory.raw_text.clone()),
                tags: memory.tags.clone(),
                labels: vec![format!("agent-memory-{}", memory.memory_layer().as_str())],
                extra_metadata: memory_metadata(memory),
                ..PutOptions::default()
            },
        )?;
        self.memvid.commit()?;
        let frame_id = self.frame_id_for_uri(&uri)?;

        let mut builder = MemoryCardBuilder::new()
            .kind(memory_kind(memory))
            .entity(memory.entity.clone())
            .slot(memory.slot.clone())
            .value(memory.value.clone())
            .source(frame_id, Some(uri))
            .engine("agent_memory", "1");
        if let Some(event_at) = memory.event_at {
            builder = builder.event_date(event_at.timestamp());
        }
        if let Some(valid_from) = memory.valid_from {
            builder = builder.document_date(valid_from.timestamp());
        } else {
            builder = builder.document_date(memory.stored_at.timestamp());
        }
        builder = builder.confidence(memory.confidence);
        builder = if memory.is_retraction {
            builder.retracts()
        } else if self
            .memvid
            .get_current_memory(&memory.entity, &memory.slot)
            .is_some()
        {
            builder.updates()
        } else {
            builder
        };

        let card = builder.build(0).map_err(|err| AgentMemoryError::Store {
            reason: err.to_string(),
        })?;
        self.memvid.put_memory_card(card)?;
        self.memvid.commit()?;
        Ok(memory.memory_id.clone())
    }

    fn touch_memory_access(&mut self, memory_id: &str, accessed_at: DateTime<Utc>) -> Result<()> {
        self.touch_memory_accesses(&[(memory_id.to_string(), accessed_at)])
    }

    fn touch_memory_accesses(&mut self, touches: &[(String, DateTime<Utc>)]) -> Result<()> {
        if !self.persist_access_touches {
            return Ok(());
        }
        let mut pending = Vec::new();

        for (memory_id, accessed_at, occurrences) in aggregate_batch_touches(touches) {
            let Some(memory) = self.get_memory(&memory_id)? else {
                continue;
            };
            let retrieval_count = memory.retrieval_count().saturating_add(occurrences);
            let uri = Self::access_touch_uri(&memory_id, accessed_at);

            self.memvid.put_bytes_with_options(
                b"access_touch",
                PutOptions {
                    timestamp: Some(accessed_at.timestamp()),
                    track: Some(TRACK_SYSTEM.to_string()),
                    kind: Some("agent_memory_access_touch".to_string()),
                    uri: Some(uri.clone()),
                    search_text: Some(format!("access touch {memory_id}")),
                    extra_metadata: BTreeMap::from([
                        ("agent_memory_id".to_string(), memory_id.clone()),
                        (
                            "agent_last_accessed_at".to_string(),
                            accessed_at.to_rfc3339(),
                        ),
                        (
                            "agent_retrieval_count".to_string(),
                            retrieval_count.to_string(),
                        ),
                    ]),
                    ..PutOptions::default()
                },
            )?;
            pending.push((memory_id, accessed_at, retrieval_count, uri));
        }

        if pending.is_empty() {
            return Ok(());
        }

        self.memvid.commit()?;

        for (memory_id, accessed_at, _retrieval_count, uri) in &pending {
            let frame_id = self.frame_id_for_uri(uri)?;
            let card = MemoryCardBuilder::new()
                .kind(MemoryKind::Other)
                .entity(access_entity())
                .slot(memory_id.clone())
                .value(accessed_at.to_rfc3339())
                .source(frame_id, Some(uri.clone()))
                .engine("agent_memory", "1")
                .document_date(accessed_at.timestamp())
                .build(0)
                .map_err(|err| AgentMemoryError::Store {
                    reason: err.to_string(),
                })?;
            self.memvid.put_memory_card(card)?;
        }
        self.memvid.commit()?;
        for (memory_id, accessed_at, retrieval_count, _) in pending {
            self.cache_access_touch(memory_id, Some((accessed_at, retrieval_count)));
        }
        Ok(())
    }

    fn update_belief(&mut self, belief: &BeliefRecord) -> Result<()> {
        let belief_json = serde_json::to_string(belief)?;
        let uri = format!("mv2://agent-memory/belief/{}", belief.belief_id);
        self.memvid.put_bytes_with_options(
            belief_json.as_bytes(),
            PutOptions {
                timestamp: Some(belief.last_reviewed_at.timestamp()),
                track: Some(TRACK_BELIEF.to_string()),
                kind: Some("agent_memory_belief".to_string()),
                uri: Some(uri.clone()),
                title: Some(format!("belief:{}:{}", belief.entity, belief.slot)),
                search_text: Some(belief.current_value.clone()),
                extra_metadata: BTreeMap::from([
                    ("belief_id".to_string(), belief.belief_id.clone()),
                    (
                        "belief_status".to_string(),
                        format!("{:?}", belief.status).to_lowercase(),
                    ),
                ]),
                ..PutOptions::default()
            },
        )?;
        self.memvid.commit()?;
        let frame_id = self.frame_id_for_uri(&uri)?;
        let builder = MemoryCardBuilder::new()
            .profile()
            .entity(belief_entity(&belief.entity))
            .slot(belief.slot.clone())
            .value(belief_json)
            .source(frame_id, Some(uri))
            .engine("agent_memory", "1")
            .document_date(belief.last_reviewed_at.timestamp())
            .confidence(belief.confidence);
        let builder = if belief.status == BeliefStatus::Retracted {
            builder.retracts()
        } else if self
            .memvid
            .get_current_memory(&belief_entity(&belief.entity), &belief.slot)
            .is_some()
        {
            builder.updates()
        } else {
            builder
        };
        let card = builder.build(0).map_err(|err| AgentMemoryError::Store {
            reason: err.to_string(),
        })?;
        self.memvid.put_memory_card(card)?;
        self.memvid.commit()?;
        Ok(())
    }

    fn get_active_belief(&mut self, entity: &str, slot: &str) -> Result<Option<BeliefRecord>> {
        let mut beliefs: Vec<_> = self
            .memvid
            .memories()
            .get_cards(&belief_entity(entity), slot)
            .into_iter()
            .filter(|card| !card.is_retracted())
            .collect();
        beliefs.sort_by_key(|card| card.effective_timestamp());
        beliefs.reverse();
        for card in beliefs {
            let belief: BeliefRecord = serde_json::from_str(&card.value)?;
            if belief.status == BeliefStatus::Stale {
                continue;
            }
            return Ok((belief.status == BeliefStatus::Active).then_some(belief));
        }
        Ok(None)
    }

    fn get_current_belief(&mut self, entity: &str, slot: &str) -> Result<Option<BeliefRecord>> {
        let mut beliefs: Vec<_> = self
            .memvid
            .memories()
            .get_cards(&belief_entity(entity), slot)
            .into_iter()
            .filter(|card| !card.is_retracted())
            .collect();
        beliefs.sort_by_key(|card| card.effective_timestamp());
        beliefs.reverse();
        for card in beliefs {
            let belief: BeliefRecord = serde_json::from_str(&card.value)?;
            if belief.status != BeliefStatus::Stale {
                return Ok(Some(belief));
            }
        }
        Ok(None)
    }

    fn get_belief_by_id(&mut self, belief_id: &str) -> Result<Option<BeliefRecord>> {
        let mut latest: Option<(i64, BeliefRecord)> = None;
        for card in self.memvid.memories().cards() {
            if !card.entity.starts_with(BELIEF_PREFIX) || card.is_retracted() {
                continue;
            }
            let belief: BeliefRecord = serde_json::from_str(&card.value)?;
            if belief.belief_id != belief_id {
                continue;
            }
            let timestamp = card.effective_timestamp();
            if latest
                .as_ref()
                .is_none_or(|(current_timestamp, _)| timestamp > *current_timestamp)
            {
                latest = Some((timestamp, belief));
            }
        }
        Ok(latest.map(|(_, belief)| belief))
    }

    fn get_memory(&mut self, memory_id: &str) -> Result<Option<DurableMemory>> {
        let cards: Vec<_> = self.memvid.memories().cards().to_vec();
        let mut latest = None;

        for card in &cards {
            if card.entity.starts_with(BELIEF_PREFIX) || is_reserved_system_entity(&card.entity) {
                continue;
            }
            let frame = self.memvid.frame_by_id(card.source_frame_id)?;
            if frame
                .extra_metadata
                .get("agent_memory_id")
                .map(String::as_str)
                != Some(memory_id)
            {
                continue;
            }
            let memory =
                self.build_durable_from_frame(card.source_frame_id, &frame.extra_metadata)?;
            if latest
                .as_ref()
                .is_none_or(|existing| memory_is_newer(&memory, existing))
            {
                latest = Some(memory);
            }
        }

        latest.map_or(Ok(None), |memory| self.apply_access_touch(memory).map(Some))
    }

    fn search(&mut self, query: &RetrievalQuery) -> Result<Vec<RetrievalHit>> {
        let mut hits = self.search_memory_cards(query)?;
        let response = self.memvid.search(SearchRequest {
            query: query.query_text.clone(),
            top_k: query.top_k.saturating_mul(4).max(10),
            snippet_chars: 200,
            uri: None,
            scope: None,
            cursor: None,
            as_of_frame: None,
            // Do not apply backend timestamp filtering here: `as_of_ts` is evaluated
            // against the underlying frame/version timestamp, which can move when a
            // memory is touched or feedback-updated. For non-historical search, `as_of`
            // visibility must be anchored on immutable ingest time (`stored_at`), so we
            // overfetch candidates and rely on the Rust-side filtering below.
            as_of_ts: None,
            no_sketch: false,
            acl_context: None,
            acl_enforcement_mode: AclEnforcementMode::default(),
        })?;
        for hit in response.hits {
            let Some(metadata) = hit.metadata else {
                continue;
            };
            if metadata.track.as_deref() != Some(TRACK_MEMORY)
                && metadata.track.as_deref() != Some(TRACK_TRACE)
            {
                continue;
            }
            let extra = metadata.extra_metadata;
            if metadata.track.as_deref() == Some(TRACK_MEMORY)
                && extra.contains_key("agent_memory_id")
            {
                let memory_id = extra.get("agent_memory_id").cloned();
                let expired = memory_id.as_deref().is_some_and(|id| self.is_expired(id));
                let memory_type = parse_memory_type(extra.get("agent_memory_type"));
                let as_of_anchor = parse_datetime(extra.get("agent_event_at"))?
                    .or(parse_datetime(extra.get("agent_valid_from"))?)
                    .or(parse_datetime(extra.get("agent_stored_at"))?)
                    .unwrap_or_else(|| query.as_of.unwrap_or_else(Utc::now));
                if let Some(as_of) = query.as_of
                    && match query.intent {
                        QueryIntent::HistoricalFact | QueryIntent::EpisodicRecall => {
                            as_of_anchor > as_of
                        }
                        _ => {
                            parse_datetime(extra.get("agent_stored_at"))?.unwrap_or(as_of_anchor)
                                > as_of
                        }
                    }
                {
                    continue;
                }
                let timestamp = parse_datetime(extra.get("agent_event_at"))?
                    .or(parse_datetime(extra.get("agent_valid_from"))?)
                    .or(parse_datetime(extra.get("agent_stored_at"))?)
                    .unwrap_or_else(|| query.as_of.unwrap_or_else(Utc::now));
                hits.push(RetrievalHit {
                    memory_id: memory_id.clone(),
                    belief_id: None,
                    entity: extra.get("agent_entity").cloned(),
                    slot: extra.get("agent_slot").cloned(),
                    value: extra.get("agent_value").cloned(),
                    text: hit.chunk_text.unwrap_or(hit.text),
                    memory_layer: Some(parse_memory_layer(
                        extra.get("agent_memory_layer"),
                        memory_type,
                    )),
                    memory_type: Some(memory_type),
                    score: hit.score.unwrap_or(0.1),
                    timestamp,
                    scope: Some(parse_scope(extra.get("agent_scope"))),
                    source: Some(parse_source_type(extra.get("agent_source_type"))),
                    from_belief: false,
                    expired,
                    metadata: {
                        let mut metadata: BTreeMap<String, String> = extra
                            .iter()
                            .filter_map(|(key, value)| {
                                key.strip_prefix("agent_meta_")
                                    .map(|short| (short.to_string(), value.clone()))
                            })
                            .collect();
                        if let Some(layer) = extra.get("agent_memory_layer") {
                            metadata.insert("memory_layer".to_string(), layer.clone());
                        }
                        if let Some(source_id) = extra.get("agent_source_id") {
                            metadata.insert("source_id".to_string(), source_id.clone());
                        }
                        if let Some(source_type) = extra.get("agent_source_type") {
                            metadata.insert("source_type".to_string(), source_type.clone());
                        }
                        if let Some(source_weight) = extra.get("agent_source_weight") {
                            metadata.insert("source_weight".to_string(), source_weight.clone());
                        }
                        if let Some(confidence) = extra.get("agent_confidence") {
                            metadata.insert("confidence".to_string(), confidence.clone());
                        }
                        if let Some(salience) = extra.get("agent_salience") {
                            metadata.insert("salience".to_string(), salience.clone());
                        }
                        if let Some(stored_at) = extra.get("agent_stored_at") {
                            metadata.insert("stored_at".to_string(), stored_at.clone());
                        }
                        if let Some(updated_at) = extra.get("agent_updated_at") {
                            metadata.insert("updated_at".to_string(), updated_at.clone());
                        }
                        if let Some(event_at) = extra.get("agent_event_at") {
                            metadata.insert("event_at".to_string(), event_at.clone());
                        }
                        if let Some(memory_id) = memory_id.as_deref()
                            && let Some((last_accessed_at, retrieval_count)) =
                                self.latest_access_touch(memory_id)?
                        {
                            metadata.insert(
                                "last_accessed_at".to_string(),
                                last_accessed_at.to_rfc3339(),
                            );
                            metadata
                                .insert("retrieval_count".to_string(), retrieval_count.to_string());
                            let version_timestamp = parse_datetime(extra.get("agent_updated_at"))?
                                .or(parse_datetime(extra.get("agent_stored_at"))?)
                                .unwrap_or(last_accessed_at)
                                .max(last_accessed_at);
                            metadata
                                .insert("updated_at".to_string(), version_timestamp.to_rfc3339());
                        }
                        if let Some(valid_from) = extra.get("agent_valid_from") {
                            metadata.insert("valid_from".to_string(), valid_from.clone());
                        }
                        if let Some(valid_to) = extra.get("agent_valid_to") {
                            metadata.insert("valid_to".to_string(), valid_to.clone());
                        }
                        metadata
                    },
                });
            } else if metadata.track.as_deref() == Some(TRACK_TRACE) {
                let mut trace_metadata = extra;
                if let Some(layer) = trace_metadata.get("memory_layer").cloned() {
                    trace_metadata.insert("memory_layer".to_string(), layer);
                }
                hits.push(RetrievalHit {
                    memory_id: trace_metadata.get("agent_trace_id").cloned(),
                    belief_id: None,
                    entity: trace_metadata.get("entity").cloned(),
                    slot: trace_metadata.get("slot").cloned(),
                    value: trace_metadata.get("value").cloned(),
                    text: hit.chunk_text.unwrap_or(hit.text),
                    memory_layer: Some(MemoryLayer::Trace),
                    memory_type: Some(MemoryType::Trace),
                    score: hit.score.unwrap_or(0.05),
                    timestamp: parse_datetime(trace_metadata.get("occurred_at"))?
                        .unwrap_or_else(|| query.as_of.unwrap_or_else(Utc::now)),
                    scope: query.scope,
                    source: trace_metadata
                        .get("source_type")
                        .map(|value| parse_source_type(Some(value))),
                    from_belief: false,
                    expired: false,
                    metadata: trace_metadata,
                });
            }
        }
        Ok(dedup_search_hits(hits))
    }

    fn list_memories_by_layer(&mut self, layer: MemoryLayer) -> Result<Vec<DurableMemory>> {
        latest_memories_by_id(self.list_memory_versions_by_layer(layer)?)
            .into_iter()
            .map(|memory| self.apply_access_touch(memory))
            .collect()
    }

    fn list_memory_versions_by_layer(&mut self, layer: MemoryLayer) -> Result<Vec<DurableMemory>> {
        let cards: Vec<_> = self.memvid.memories().cards().to_vec();
        let mut memories = Vec::new();
        for card in &cards {
            if card.entity.starts_with(BELIEF_PREFIX) || is_reserved_system_entity(&card.entity) {
                continue;
            }
            let frame = self.memvid.frame_by_id(card.source_frame_id)?;
            if !frame.extra_metadata.contains_key("agent_memory_id") {
                continue;
            }
            let memory =
                self.build_durable_from_frame(card.source_frame_id, &frame.extra_metadata)?;
            if memory.memory_layer() == layer {
                memories.push(memory);
            }
        }
        Ok(memories)
    }

    fn list_memories_for_belief(&mut self, entity: &str, slot: &str) -> Result<Vec<DurableMemory>> {
        Ok(self
            .list_memory_versions_by_layer(MemoryLayer::Belief)?
            .into_iter()
            .filter(|memory| memory.entity == entity && memory.slot == slot)
            .collect())
    }

    fn expire_memory(&mut self, memory_id: &str) -> Result<()> {
        let now = self.clock.now();
        let uri = format!("mv2://agent-memory/expiry/{memory_id}");
        self.memvid.put_bytes_with_options(
            format!("expired {memory_id}").as_bytes(),
            PutOptions {
                timestamp: Some(now.timestamp()),
                track: Some(TRACK_SYSTEM.to_string()),
                kind: Some("agent_memory_expiry".to_string()),
                uri: Some(uri.clone()),
                search_text: Some(format!("expired {memory_id}")),
                ..PutOptions::default()
            },
        )?;
        self.memvid.commit()?;
        let frame_id = self.frame_id_for_uri(&uri)?;
        let card = MemoryCardBuilder::new()
            .kind(MemoryKind::Other)
            .entity(expiry_entity())
            .slot(memory_id.to_string())
            .value("expired".to_string())
            .source(frame_id, Some(uri))
            .engine("agent_memory", "1")
            .document_date(now.timestamp())
            .build(0)
            .map_err(|err| AgentMemoryError::Store {
                reason: err.to_string(),
            })?;
        self.memvid.put_memory_card(card)?;
        self.memvid.commit()?;
        Ok(())
    }
}
