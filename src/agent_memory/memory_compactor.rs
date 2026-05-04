use std::collections::BTreeMap;

use uuid::Uuid;

use super::adapters::memvid_store::MemoryStore;
use super::clock::Clock;
use super::enums::{MemoryLayer, MemoryType, Scope, SourceType};
use super::errors::Result;
use super::ontology::OntologyRegistry;
use super::schemas::{DurableMemory, Provenance};

/// Compaction strategy to apply during a governed-memory maintenance pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionMode {
    /// Remove lower-confidence duplicates that share the same (entity, slot, value).
    Dedupe,
    /// Scan episode-layer memories and synthesise Belief-layer summaries for entities
    /// that have accumulated more than 10 episodic observations.
    Summarize,
    /// Promote high-confidence Trace-layer memories to the Episode layer and expire
    /// the originating traces.
    Distill,
    /// Run the concept-synthesis pass: cluster beliefs into concept nodes, build
    /// entity profiles, mine procedure sequences, detect recurring patterns, and
    /// produce project-scoped summaries.
    Synthesize,
}

impl Default for CompactionMode {
    fn default() -> Self {
        Self::Dedupe
    }
}

/// Summary of what a compaction pass accomplished.
#[derive(Debug, Clone, Default)]
pub struct CompactionResult {
    pub mode: CompactionMode,
    pub deduplicated_count: usize,
    pub summaries_created: usize,
    pub distilled_count: usize,
    /// Memory IDs consumed or expired during the pass.
    pub source_memory_ids: Vec<String>,
    /// Memory IDs created during the pass.
    pub created_memory_ids: Vec<String>,
}

/// Governed-memory compaction engine with three operational modes.
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryCompactor;

impl MemoryCompactor {
    /// Run the requested compaction strategy against `store`.
    pub fn compact<S: MemoryStore>(
        &self,
        store: &mut S,
        mode: CompactionMode,
        clock: &dyn Clock,
        ontology: &mut OntologyRegistry,
    ) -> Result<CompactionResult> {
        match mode {
            CompactionMode::Dedupe => self.compact_dedupe(store),
            CompactionMode::Summarize => self.compact_summarize(store, clock),
            CompactionMode::Distill => self.compact_distill(store, clock),
            CompactionMode::Synthesize => {
                use super::concept_synthesis::ConceptSynthesizer;
                let synthesis = ConceptSynthesizer.synthesize(store, clock, ontology, None)?;
                Ok(CompactionResult {
                    mode: CompactionMode::Synthesize,
                    summaries_created: synthesis.concepts_created
                        + synthesis.profiles_updated
                        + synthesis.procedures_mined
                        + synthesis.patterns_found
                        + synthesis.project_summaries,
                    created_memory_ids: synthesis.created_memory_ids,
                    ..Default::default()
                })
            }
        }
    }

    // ------------------------------------------------------------------
    // Dedupe — remove lower-confidence exact duplicates
    // ------------------------------------------------------------------

    fn compact_dedupe<S: MemoryStore>(&self, store: &mut S) -> Result<CompactionResult> {
        let mut result = CompactionResult {
            mode: CompactionMode::Dedupe,
            ..Default::default()
        };

        for layer in [
            MemoryLayer::Trace,
            MemoryLayer::Episode,
            MemoryLayer::Belief,
        ] {
            let memories = store.list_memories_by_layer(layer)?;

            // Group by (entity_lower, slot_lower)
            let mut groups: BTreeMap<(String, String), Vec<DurableMemory>> = BTreeMap::new();
            for memory in memories {
                let key = (memory.entity.to_lowercase(), memory.slot.to_lowercase());
                groups.entry(key).or_default().push(memory);
            }

            for (_, mut group) in groups {
                if group.len() < 2 {
                    continue;
                }
                // Sub-group by lowercased value
                let mut value_groups: BTreeMap<String, Vec<DurableMemory>> = BTreeMap::new();
                for memory in group.drain(..) {
                    value_groups
                        .entry(memory.value.to_lowercase())
                        .or_default()
                        .push(memory);
                }

                for (_, mut same_value) in value_groups {
                    if same_value.len() < 2 {
                        continue;
                    }
                    // Highest-confidence first; keep index 0, expire the rest
                    same_value.sort_by(|a, b| {
                        b.confidence
                            .partial_cmp(&a.confidence)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    for dup in same_value.into_iter().skip(1) {
                        result.source_memory_ids.push(dup.memory_id.clone());
                        store.expire_memory(&dup.memory_id)?;
                        result.deduplicated_count += 1;
                    }
                }
            }
        }

        Ok(result)
    }

    // ------------------------------------------------------------------
    // Summarize — synthesise Belief-layer memories from repeat episodes
    // ------------------------------------------------------------------

    fn compact_summarize<S: MemoryStore>(
        &self,
        store: &mut S,
        clock: &dyn Clock,
    ) -> Result<CompactionResult> {
        let mut result = CompactionResult {
            mode: CompactionMode::Summarize,
            ..Default::default()
        };
        let episodes = store.list_memories_by_layer(MemoryLayer::Episode)?;

        // Group episodes by entity
        let mut entity_groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (idx, ep) in episodes.iter().enumerate() {
            entity_groups
                .entry(ep.entity.clone())
                .or_default()
                .push(idx);
        }

        for (entity, indices) in entity_groups {
            if indices.len() <= 10 {
                continue;
            }
            let total = indices.len();

            // Find the most frequent (slot, value) pair
            let mut freq: BTreeMap<(String, String), usize> = BTreeMap::new();
            for &idx in &indices {
                let ep = &episodes[idx];
                *freq.entry((ep.slot.clone(), ep.value.clone())).or_insert(0) += 1;
            }
            let Some(((slot, value), count)) = freq.into_iter().max_by_key(|(_, c)| *c) else {
                continue;
            };

            let confidence = count as f32 / total as f32;
            let now = clock.now();
            let memory_id = Uuid::new_v4().to_string();

            let new_memory = DurableMemory {
                memory_id: memory_id.clone(),
                candidate_id: Uuid::new_v4().to_string(),
                stored_at: now,
                updated_at: None,
                entity,
                slot,
                value: value.clone(),
                raw_text: value,
                memory_type: MemoryType::Fact,
                confidence,
                salience: 0.7,
                scope: Scope::Private,
                ttl: None,
                source: Provenance {
                    source_type: SourceType::System,
                    source_id: "memory_compactor".to_string(),
                    source_label: Some("summarize".to_string()),
                    observed_by: None,
                    trust_weight: confidence,
                },
                event_at: None,
                valid_from: None,
                valid_to: None,
                internal_layer: Some(MemoryLayer::Belief),
                tags: vec!["compacted".to_string(), "summary".to_string()],
                metadata: BTreeMap::from([(
                    "compaction_mode".to_string(),
                    "summarize".to_string(),
                )]),
                is_retraction: false,
            };

            store.put_memory(&new_memory)?;
            result.created_memory_ids.push(memory_id);
            result.summaries_created += 1;
        }

        Ok(result)
    }

    // ------------------------------------------------------------------
    // Distill — promote high-confidence Trace memories to Episode layer
    // ------------------------------------------------------------------

    fn compact_distill<S: MemoryStore>(
        &self,
        store: &mut S,
        clock: &dyn Clock,
    ) -> Result<CompactionResult> {
        let mut result = CompactionResult {
            mode: CompactionMode::Distill,
            ..Default::default()
        };
        let traces = store.list_memories_by_layer(MemoryLayer::Trace)?;

        for trace in traces {
            if trace.confidence <= 0.5 {
                continue;
            }

            let now = clock.now();
            let memory_id = Uuid::new_v4().to_string();

            let mut metadata = trace.metadata.clone();
            metadata.insert("distilled_from".to_string(), trace.memory_id.clone());

            let new_memory = DurableMemory {
                memory_id: memory_id.clone(),
                candidate_id: Uuid::new_v4().to_string(),
                stored_at: now,
                updated_at: None,
                entity: trace.entity.clone(),
                slot: trace.slot.clone(),
                value: trace.value.clone(),
                raw_text: trace.raw_text.clone(),
                memory_type: MemoryType::Episode,
                confidence: trace.confidence,
                salience: trace.salience,
                scope: trace.scope,
                ttl: None,
                source: trace.source.clone(),
                event_at: trace.event_at,
                valid_from: None,
                valid_to: None,
                internal_layer: Some(MemoryLayer::Episode),
                tags: trace.tags.clone(),
                metadata,
                is_retraction: false,
            };

            store.put_memory(&new_memory)?;
            store.expire_memory(&trace.memory_id)?;
            result.created_memory_ids.push(memory_id);
            result.source_memory_ids.push(trace.memory_id);
            result.distilled_count += 1;
        }

        Ok(result)
    }

    // ------------------------------------------------------------------
    // Legacy surface used by run_maintenance()
    // ------------------------------------------------------------------

    /// Returns `true` — compaction is now operational.
    #[must_use]
    pub const fn is_supported(&self) -> bool {
        true
    }

    /// Current compaction status string.
    #[must_use]
    pub const fn status(&self) -> &'static str {
        "active"
    }

    /// Reason string shown when compaction is unsupported. Empty when supported.
    #[must_use]
    pub const fn unsupported_reason(&self) -> &'static str {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_memory::adapters::memvid_store::InMemoryMemoryStore;
    use crate::agent_memory::clock::FixedClock;
    use crate::agent_memory::enums::SourceType;
    use crate::agent_memory::ontology::OntologyRegistry;
    use crate::agent_memory::schemas::Provenance;
    use chrono::TimeZone;

    fn clock() -> FixedClock {
        FixedClock::new(chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap())
    }

    fn store() -> InMemoryMemoryStore {
        InMemoryMemoryStore::default()
    }

    fn make_memory(
        id: &str,
        entity: &str,
        slot: &str,
        value: &str,
        confidence: f32,
        layer: MemoryLayer,
        memory_type: MemoryType,
    ) -> DurableMemory {
        DurableMemory {
            memory_id: id.to_string(),
            candidate_id: format!("cand-{id}"),
            stored_at: chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            updated_at: None,
            entity: entity.to_string(),
            slot: slot.to_string(),
            value: value.to_string(),
            raw_text: format!("{entity} {slot} {value}"),
            memory_type,
            confidence,
            salience: 0.8,
            scope: Scope::Private,
            ttl: None,
            source: Provenance {
                source_type: SourceType::Chat,
                source_id: "test".to_string(),
                source_label: None,
                observed_by: None,
                trust_weight: 0.9,
            },
            event_at: None,
            valid_from: None,
            valid_to: None,
            internal_layer: Some(layer),
            tags: vec![],
            metadata: BTreeMap::new(),
            is_retraction: false,
        }
    }

    #[test]
    fn dedupe_removes_lower_confidence_duplicate() {
        let mut s = store();
        let high = make_memory(
            "m1",
            "user",
            "lang",
            "rust",
            0.9,
            MemoryLayer::Belief,
            MemoryType::Fact,
        );
        let low = make_memory(
            "m2",
            "user",
            "lang",
            "rust",
            0.4,
            MemoryLayer::Belief,
            MemoryType::Fact,
        );
        s.put_memory(&high).unwrap();
        s.put_memory(&low).unwrap();

        let result = MemoryCompactor
            .compact(
                &mut s,
                CompactionMode::Dedupe,
                &clock(),
                &mut OntologyRegistry::new(),
            )
            .unwrap();

        assert_eq!(result.deduplicated_count, 1);
        assert!(result.source_memory_ids.contains(&"m2".to_string()));
        // m1 should survive
        assert!(s.get_memory("m1").unwrap().is_some());
    }

    #[test]
    fn dedupe_preserves_distinct_keys() {
        let mut s = store();
        let a = make_memory(
            "m1",
            "user",
            "lang",
            "rust",
            0.9,
            MemoryLayer::Belief,
            MemoryType::Fact,
        );
        let b = make_memory(
            "m2",
            "user",
            "editor",
            "vim",
            0.8,
            MemoryLayer::Belief,
            MemoryType::Fact,
        );
        s.put_memory(&a).unwrap();
        s.put_memory(&b).unwrap();

        let result = MemoryCompactor
            .compact(
                &mut s,
                CompactionMode::Dedupe,
                &clock(),
                &mut OntologyRegistry::new(),
            )
            .unwrap();
        assert_eq!(result.deduplicated_count, 0);
    }

    #[test]
    fn summarize_creates_belief_from_repeated_episodes() {
        let mut s = store();
        // Insert 11 episodes for the same entity with the same slot+value
        for i in 0..11 {
            let m = make_memory(
                &format!("m{i}"),
                "user",
                "prefers",
                "dark_mode",
                0.8,
                MemoryLayer::Episode,
                MemoryType::Episode,
            );
            s.put_memory(&m).unwrap();
        }

        let result = MemoryCompactor
            .compact(
                &mut s,
                CompactionMode::Summarize,
                &clock(),
                &mut OntologyRegistry::new(),
            )
            .unwrap();
        assert_eq!(result.summaries_created, 1);
        assert_eq!(result.created_memory_ids.len(), 1);

        // The created memory should exist in the store
        let created_id = &result.created_memory_ids[0];
        assert!(s.get_memory(created_id).unwrap().is_some());
    }

    #[test]
    fn summarize_skips_entity_with_few_episodes() {
        let mut s = store();
        for i in 0..5 {
            let m = make_memory(
                &format!("m{i}"),
                "user",
                "prefers",
                "dark_mode",
                0.8,
                MemoryLayer::Episode,
                MemoryType::Episode,
            );
            s.put_memory(&m).unwrap();
        }

        let result = MemoryCompactor
            .compact(
                &mut s,
                CompactionMode::Summarize,
                &clock(),
                &mut OntologyRegistry::new(),
            )
            .unwrap();
        assert_eq!(result.summaries_created, 0);
    }

    #[test]
    fn distill_promotes_high_confidence_trace() {
        let mut s = store();
        let t = make_memory(
            "t1",
            "user",
            "lang",
            "rust",
            0.8,
            MemoryLayer::Trace,
            MemoryType::Trace,
        );
        s.put_memory(&t).unwrap();

        let result = MemoryCompactor
            .compact(
                &mut s,
                CompactionMode::Distill,
                &clock(),
                &mut OntologyRegistry::new(),
            )
            .unwrap();
        assert_eq!(result.distilled_count, 1);
        assert!(result.source_memory_ids.contains(&"t1".to_string()));

        // The new episode-layer memory should exist
        assert_eq!(result.created_memory_ids.len(), 1);
        let new_id = &result.created_memory_ids[0];
        let new_mem = s.get_memory(new_id).unwrap().unwrap();
        assert_eq!(
            new_mem.metadata.get("distilled_from").map(String::as_str),
            Some("t1")
        );
    }

    #[test]
    fn distill_ignores_low_confidence_trace() {
        let mut s = store();
        let t = make_memory(
            "t1",
            "user",
            "lang",
            "rust",
            0.3,
            MemoryLayer::Trace,
            MemoryType::Trace,
        );
        s.put_memory(&t).unwrap();

        let result = MemoryCompactor
            .compact(
                &mut s,
                CompactionMode::Distill,
                &clock(),
                &mut OntologyRegistry::new(),
            )
            .unwrap();
        assert_eq!(result.distilled_count, 0);
    }
}
