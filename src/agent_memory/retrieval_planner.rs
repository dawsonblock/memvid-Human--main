//! Multi-pool retrieval planner that aggregates candidates from lexical, vector,
//! metadata, time, and correction pools before handing off to the ranker.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use super::adapters::memvid_store::MemoryStore;
use super::clock::Clock;
use super::enums::{MemoryLayer, MemoryType, Scope};
use super::errors::Result;
use super::policy::PolicyProfile;
use super::ranker::Ranker;
use super::retrieval_candidates::{CandidatePool, CandidateScores, RetrievalCandidate};
use super::schemas::{DurableMemory, RetrievalHit, RetrievalQuery};
use super::semantic_retrieval::SemanticRetriever;

/// All durable layers except Correction (handled by its own dedicated pool).
const DURABLE_LAYERS: &[MemoryLayer] = &[
    MemoryLayer::Trace,
    MemoryLayer::Episode,
    MemoryLayer::Belief,
    MemoryLayer::GoalState,
    MemoryLayer::SelfModel,
    MemoryLayer::Procedure,
];

/// All seven layers, including Correction, used by the time pool.
const ALL_LAYERS: &[MemoryLayer] = &[
    MemoryLayer::Trace,
    MemoryLayer::Episode,
    MemoryLayer::Belief,
    MemoryLayer::GoalState,
    MemoryLayer::SelfModel,
    MemoryLayer::Procedure,
    MemoryLayer::Correction,
];

/// Per-pool hit counts and availability flags captured during planning.
#[derive(Debug, Clone, Default)]
pub struct PoolStats {
    pub lexical_count: usize,
    pub vector_count: usize,
    pub metadata_count: usize,
    pub time_count: usize,
    pub correction_count: usize,
    pub vector_pool_available: bool,
    pub vector_pool_skipped_reason: Option<String>,
}

/// The output of a single planner invocation.
#[derive(Debug)]
pub struct PlannerResult {
    /// Ranked and truncated candidates ready for the upstream retriever.
    pub candidates: Vec<RetrievalCandidate>,
    /// Per-pool diagnostic counts.
    pub pool_stats: PoolStats,
}

/// Stateless multi-pool retrieval planner.
#[derive(Debug, Default, Clone)]
pub struct RetrievalPlanner {
    ranker: Ranker,
}

impl RetrievalPlanner {
    /// Build a planner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Run the five-pool retrieval pipeline and return ranked candidates.
    ///
    /// # Contract
    /// This method is **strictly read-only**: it never calls `put_memory()`,
    /// `touch_memory_access()`, or any other mutating store method.
    pub fn plan<S: MemoryStore>(
        &self,
        store: &mut S,
        query: &RetrievalQuery,
        clock: &dyn Clock,
    ) -> Result<PlannerResult> {
        let now: DateTime<Utc> = clock.now();
        let mut stats = PoolStats::default();
        stats.vector_pool_available = false;
        stats.vector_pool_skipped_reason =
            Some("no independent vector index/provider configured".to_string());

        // Accumulate hits keyed by memory_id → (hit, pools).
        let mut merged: HashMap<String, (RetrievalHit, Vec<CandidatePool>)> = HashMap::new();

        // ── Pool 1: Lexical (store.search) ──────────────────────────────────
        let lexical_hits = store.search(query)?;
        stats.lexical_count += lexical_hits.len();
        for hit in lexical_hits {
            let key = hit_key(&hit);
            merged
                .entry(key)
                .or_insert_with(|| (hit, Vec::new()))
                .1
                .push(CandidatePool::Lexical);
        }

        // ── Pool 2: TF-IDF / synonym expansion (SemanticRetriever) ──────────
        let tfidf_hits =
            SemanticRetriever::semantic_hits(&query.query_text, store, query.top_k, query)?;
        stats.lexical_count += tfidf_hits.len();
        for hit in tfidf_hits {
            let key = hit_key(&hit);
            let entry = merged
                .entry(key)
                .or_insert_with(|| (hit.clone(), Vec::new()));
            if !entry.1.contains(&CandidatePool::Lexical) {
                entry.1.push(CandidatePool::Lexical);
            }
        }

        // ── Pool 3: Metadata (entity/slot scan) ─────────────────────────────
        if query.entity.is_some() || query.slot.is_some() {
            for &layer in DURABLE_LAYERS {
                let memories = store.list_memories_by_layer(layer)?;
                for mem in memories {
                    let entity_match = query.entity.as_deref().map_or(true, |e| mem.entity == e);
                    let slot_match = query.slot.as_deref().map_or(true, |s| mem.slot == s);
                    if entity_match && slot_match {
                        stats.metadata_count += 1;
                        let hit = durable_to_hit(&mem, now);
                        let key = hit_key(&hit);
                        let entry = merged.entry(key).or_insert_with(|| (hit, Vec::new()));
                        if !entry.1.contains(&CandidatePool::Metadata) {
                            entry.1.push(CandidatePool::Metadata);
                        }
                    }
                }
            }
        }

        // ── Pool 4: Time (as_of historical scan) ────────────────────────────
        if let Some(as_of) = query.as_of {
            for &layer in ALL_LAYERS {
                let versions = store.list_memory_versions_by_layer(layer)?;
                for mem in versions {
                    let stored = mem.stored_at;
                    if stored <= as_of {
                        stats.time_count += 1;
                        let hit = durable_to_hit(&mem, now);
                        let key = hit_key(&hit);
                        let entry = merged.entry(key).or_insert_with(|| (hit, Vec::new()));
                        if !entry.1.contains(&CandidatePool::Time) {
                            entry.1.push(CandidatePool::Time);
                        }
                    }
                }
            }
        }

        // ── Pool 5: Corrections ──────────────────────────────────────────────
        if let (Some(entity), Some(slot)) = (&query.entity, &query.slot) {
            let corrections = store.get_corrections_by_entity_slot(entity, slot)?;
            stats.correction_count += corrections.len();
            for mem in corrections {
                let hit = durable_to_hit(&mem, now);
                let key = hit_key(&hit);
                let entry = merged.entry(key).or_insert_with(|| (hit, Vec::new()));
                if !entry.1.contains(&CandidatePool::Correction) {
                    entry.1.push(CandidatePool::Correction);
                }
            }
        }

        // ── Rank merged hits ─────────────────────────────────────────────────
        let profile = PolicyProfile::default();
        let weights = profile.soft_weights();

        let hits_vec: Vec<RetrievalHit> = merged.values().map(|(h, _)| h.clone()).collect();
        let ranked = self
            .ranker
            .rerank_with_weights(hits_vec, query.intent, now, weights);

        // ── Build candidates (truncated to top_k) ────────────────────────────
        let candidates: Vec<RetrievalCandidate> = ranked
            .into_iter()
            .take(query.top_k)
            .map(|hit| {
                let key = hit_key(&hit);
                let pools = merged.get(&key).map(|(_, p)| p.clone()).unwrap_or_default();
                let layer = hit.memory_layer.unwrap_or(MemoryLayer::Trace);
                let scores = scores_from_hit(&hit, now, query.scope);
                RetrievalCandidate {
                    memory_id: hit.memory_id.clone().unwrap_or_default(),
                    layer,
                    source_pools: pools,
                    scores,
                    hit,
                    reason: Vec::new(),
                }
            })
            .collect();

        Ok(PlannerResult {
            candidates,
            pool_stats: stats,
        })
    }
}

// ── Private helpers ──────────────────────────────────────────────────────────

/// Stable key for de-duplicating merged hits.
fn hit_key(hit: &RetrievalHit) -> String {
    hit.memory_id
        .clone()
        .unwrap_or_else(|| format!("{}|{}", hit.text.len(), hit.timestamp.timestamp()))
}

/// Convert a `DurableMemory` to a minimal `RetrievalHit` for metadata / time / correction pools.
fn durable_to_hit(memory: &DurableMemory, now: DateTime<Utc>) -> RetrievalHit {
    let mut metadata = memory.metadata.clone();
    metadata.insert(
        "_planner_confidence".to_string(),
        memory.confidence.to_string(),
    );
    metadata.insert(
        "_planner_source_trust".to_string(),
        memory.source.trust_weight.to_string(),
    );
    RetrievalHit {
        memory_id: Some(memory.memory_id.clone()),
        belief_id: None,
        entity: memory.entity_non_empty().map(ToString::to_string),
        slot: memory.slot_non_empty().map(ToString::to_string),
        value: memory.value_non_empty().map(ToString::to_string),
        text: memory.raw_text.clone(),
        memory_layer: Some(memory.memory_layer()),
        memory_type: Some(memory.memory_type),
        score: memory.salience,
        timestamp: memory.event_timestamp(),
        scope: Some(memory.scope),
        source: Some(memory.source.source_type),
        from_belief: false,
        expired: memory.ttl.map_or(false, |ttl| {
            memory.stored_at + chrono::Duration::seconds(ttl) < now
        }),
        metadata,
    }
}

/// Build a `CandidateScores` snapshot from a ranked hit.
///
/// `query_scope` is the caller's requested scope; `None` means "any scope matches".
fn scores_from_hit(
    hit: &RetrievalHit,
    now: DateTime<Utc>,
    query_scope: Option<Scope>,
) -> CandidateScores {
    let age_secs = (now - hit.timestamp).num_seconds().max(0) as f32;
    let recency = (-age_secs / 2_592_000.0_f32).exp();
    let confidence: f32 = hit
        .metadata
        .get("_planner_confidence")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let source_trust: f32 = hit
        .metadata
        .get("_planner_source_trust")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    // B1: derive scope_match from the query's requested scope vs the hit's scope.
    // Scope::Shared always passes; a missing scope on either side is permissive.
    let scope_match =
        query_scope.is_none_or(|qs| hit.scope.is_none_or(|hs| hs == Scope::Shared || hs == qs));
    // B2: surface correction provenance instead of always emitting None.
    let correction_status = if hit.memory_type == Some(MemoryType::Correction) {
        Some("correction".to_string())
    } else {
        None
    };
    CandidateScores {
        lexical_score: hit.score,
        vector_score: None,
        entity_slot_match: hit.entity.is_some() && hit.slot.is_some(),
        recency,
        salience: hit.score,
        confidence,
        source_trust,
        correction_status,
        scope_match,
    }
}
