use std::collections::BTreeMap;

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::adapters::memvid_store::MemoryStore;
use super::clock::Clock;
use super::enums::{MemoryLayer, MemoryType, Scope, SourceType};
use super::errors::Result;
use super::graph::{EdgeKind, GraphEdge, GraphEdgeStore};
use super::ontology::{ConceptCanonicalizer, OntologyRegistry};
use super::schemas::{DurableMemory, Provenance};

// ------------------------------------------------------------------
// Public types
// ------------------------------------------------------------------

/// A synthesised concept node derived from belief-layer memories for a single entity.
#[derive(Debug, Clone)]
pub struct ConceptNode {
    /// Stable 8-character ID derived from sha256(entity + "|" + concept_text).
    pub concept_id: String,
    /// The entity this concept describes.
    pub entity: String,
    /// Human-readable summary of the synthesised concept.
    pub concept_text: String,
    /// Memory IDs of the source beliefs that were clustered to produce this concept.
    pub source_belief_ids: Vec<String>,
}

/// Results returned after a synthesis pass.
#[derive(Debug, Clone, Default)]
pub struct SynthesisResult {
    /// Number of new concept nodes created.
    pub concepts_created: usize,
    /// Number of entity profiles updated (belief-layer summaries enriched).
    pub profiles_updated: usize,
    /// Number of procedure-like patterns identified across episodes.
    pub procedures_mined: usize,
    /// Number of recurring slot→value patterns detected as stable facts.
    pub patterns_found: usize,
    /// Number of project-scoped summaries produced.
    pub project_summaries: usize,
    /// Memory IDs of newly persisted concept memories.
    pub created_memory_ids: Vec<String>,
}

// ------------------------------------------------------------------
// ConceptSynthesizer
// ------------------------------------------------------------------

/// Stateless concept-synthesis engine.
///
/// Call [`ConceptSynthesizer::synthesize`] after belief-layer memories have
/// accumulated to derive higher-order concept nodes, entity profiles,
/// procedure patterns, and project summaries.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConceptSynthesizer;

impl ConceptSynthesizer {
    /// Run a full synthesis pass over `store` and return what was produced.
    ///
    /// The method is intentionally idempotent: re-running it against the same
    /// store state will not create duplicate concept nodes because the concept
    /// ID is deterministic (sha256 of entity + concept_text).
    pub fn synthesize<S: MemoryStore>(
        &self,
        store: &mut S,
        clock: &dyn Clock,
        ontology: &mut OntologyRegistry,
        mut graph: Option<&mut GraphEdgeStore>,
    ) -> Result<SynthesisResult> {
        let mut result = SynthesisResult::default();

        self.cluster_concepts(store, clock, ontology, graph, &mut result)?;
        self.synthesize_profile(store, clock, &mut result)?;
        self.mine_procedures(store, clock, &mut result)?;
        self.mine_patterns(store, clock, &mut result)?;
        self.summarize_project(store, clock, &mut result)?;

        Ok(result)
    }

    // ------------------------------------------------------------------
    // cluster_concepts — group beliefs by entity and create concept nodes
    // ------------------------------------------------------------------

    fn cluster_concepts<S: MemoryStore>(
        &self,
        store: &mut S,
        clock: &dyn Clock,
        ontology: &mut OntologyRegistry,
        mut graph: Option<&mut GraphEdgeStore>,
        result: &mut SynthesisResult,
    ) -> Result<()> {
        let beliefs = store.list_memories_by_layer(MemoryLayer::Belief)?;

        // Group beliefs by entity — exclude synthesized memories so the input
        // set is stable across passes (prevents hash drift on the concept_id).
        let mut entity_beliefs: BTreeMap<String, Vec<DurableMemory>> = BTreeMap::new();
        for b in beliefs {
            // Skip any memory we previously created via synthesis
            if b.metadata.contains_key("concept_id")
                || b.metadata.contains_key("profile_id")
                || b.metadata.contains_key("procedure_id")
                || b.metadata.contains_key("pattern_id")
                || b.metadata.contains_key("project_summary_id")
            {
                continue;
            }
            let entity = b.entity.trim().to_string();
            if entity.is_empty() {
                continue;
            }
            entity_beliefs.entry(entity).or_default().push(b);
        }

        for (entity, group) in &entity_beliefs {
            if group.len() < 2 {
                // Not enough signal for a meaningful concept cluster
                continue;
            }

            // Build a representative concept text from the highest-confidence beliefs
            let mut sorted = group.clone();
            sorted.sort_by(|a, b| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let top: Vec<&DurableMemory> = sorted.iter().take(3).collect();
            // Sort by slot so concept_text (and thus concept_id) is the same
            // regardless of HashMap iteration order across synthesis passes.
            let mut top_stable = top.clone();
            top_stable.sort_by_key(|m| m.slot.as_str());
            let concept_text = top_stable
                .iter()
                .map(|m| format!("{}={}", m.slot, m.value))
                .collect::<Vec<_>>()
                .join("; ");

            let concept_id = concept_id_for(entity, &concept_text);
            let source_belief_ids: Vec<String> =
                group.iter().map(|m| m.memory_id.clone()).collect();

            // Skip if already exists (idempotent)
            let existing = store.list_memories_by_layer(MemoryLayer::Belief)?;
            let already_exists = existing
                .iter()
                .any(|m| m.metadata.get("concept_id").map(String::as_str) == Some(&concept_id));
            if already_exists {
                continue;
            }

            let avg_confidence: f32 =
                group.iter().map(|m| m.confidence).sum::<f32>() / group.len() as f32;

            // Ontology-level dedup: if this concept text already resolves to a
            // canonical entry, add provenance and skip creating a duplicate node.
            let now_ts = clock.now().timestamp();
            if let Some(existing_cid) = ontology.resolve_alias(&concept_text).map(str::to_string) {
                for mid in &source_belief_ids {
                    ontology.add_supporting_memory(
                        &existing_cid,
                        mid.clone(),
                        Some(avg_confidence),
                        "cluster_synthesis",
                        now_ts,
                    );
                }
                continue;
            }

            let memory_id = Uuid::new_v4().to_string();
            let mut metadata = BTreeMap::new();
            metadata.insert("concept_id".to_string(), concept_id.clone());
            metadata.insert("synthesis_mode".to_string(), "cluster".to_string());
            metadata.insert("source_belief_count".to_string(), group.len().to_string());

            let node = DurableMemory {
                memory_id: memory_id.clone(),
                candidate_id: Uuid::new_v4().to_string(),
                stored_at: clock.now(),
                updated_at: None,
                entity: entity.clone(),
                slot: "concept".to_string(),
                value: concept_text.clone(),
                raw_text: concept_text.clone(),
                memory_type: MemoryType::Fact,
                confidence: avg_confidence.min(1.0),
                salience: 0.8,
                scope: Scope::Private,
                ttl: None,
                source: Provenance {
                    source_type: SourceType::System,
                    source_id: "concept_synthesizer".to_string(),
                    source_label: Some("cluster".to_string()),
                    observed_by: None,
                    trust_weight: avg_confidence.min(1.0),
                },
                event_at: None,
                valid_from: None,
                valid_to: None,
                internal_layer: Some(MemoryLayer::Belief),
                tags: vec![
                    "synthesized".to_string(),
                    "concept".to_string(),
                    "cluster".to_string(),
                ],
                metadata,
                is_retraction: false,
                thread_id: None,
                parent_memory_id: None,
            };

            store.put_memory(&node)?;
            result.created_memory_ids.push(memory_id.clone());
            result.concepts_created += 1;

            // Add DerivedFrom edges in the knowledge graph: concept node → each source belief.
            if let Some(g) = graph.as_mut() {
                for belief_id in &source_belief_ids {
                    let edge = GraphEdge::new(
                        memory_id.clone(),
                        belief_id.clone(),
                        EdgeKind::DerivedFrom,
                        avg_confidence,
                        Some(memory_id.clone()),
                        now_ts,
                    );
                    g.add_edge(edge);
                }
            }

            // Register the new concept in the ontology for future deduplication.
            let _ = ontology.register(
                concept_id.clone(),
                concept_text.clone(),
                entity.clone(),
                avg_confidence,
                now_ts,
            );
            ontology.add_supporting_memory(
                &concept_id,
                memory_id,
                Some(avg_confidence),
                "cluster_synthesis_new",
                now_ts,
            );

            let _ = ConceptNode {
                concept_id,
                entity: entity.clone(),
                concept_text,
                source_belief_ids,
            };
        }

        Ok(())
    }

    // ------------------------------------------------------------------
    // synthesize_profile — produce a consolidated entity profile belief
    // ------------------------------------------------------------------

    fn synthesize_profile<S: MemoryStore>(
        &self,
        store: &mut S,
        clock: &dyn Clock,
        result: &mut SynthesisResult,
    ) -> Result<()> {
        let beliefs = store.list_memories_by_layer(MemoryLayer::Belief)?;

        let mut entity_slots: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for b in &beliefs {
            let entity = b.entity.trim().to_string();
            if entity.is_empty() {
                continue;
            }
            entity_slots
                .entry(entity)
                .or_default()
                .insert(b.slot.clone(), b.value.clone());
        }

        for (entity, slots) in &entity_slots {
            if slots.len() < 3 {
                continue;
            }

            let profile_text = slots
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join(", ");

            let profile_id = concept_id_for(entity, &profile_text);

            // Idempotency check
            let existing = store.list_memories_by_layer(MemoryLayer::Belief)?;
            let already_exists = existing
                .iter()
                .any(|m| m.metadata.get("profile_id").map(String::as_str) == Some(&profile_id));
            if already_exists {
                continue;
            }

            let memory_id = Uuid::new_v4().to_string();
            let mut metadata = BTreeMap::new();
            metadata.insert("profile_id".to_string(), profile_id);
            metadata.insert("synthesis_mode".to_string(), "profile".to_string());
            metadata.insert("slot_count".to_string(), slots.len().to_string());

            let profile_mem = DurableMemory {
                memory_id: memory_id.clone(),
                candidate_id: Uuid::new_v4().to_string(),
                stored_at: clock.now(),
                updated_at: None,
                entity: entity.clone(),
                slot: "profile".to_string(),
                value: profile_text.clone(),
                raw_text: profile_text,
                memory_type: MemoryType::Fact,
                confidence: 0.75,
                salience: 0.85,
                scope: Scope::Private,
                ttl: None,
                source: Provenance {
                    source_type: SourceType::System,
                    source_id: "concept_synthesizer".to_string(),
                    source_label: Some("profile".to_string()),
                    observed_by: None,
                    trust_weight: 0.75,
                },
                event_at: None,
                valid_from: None,
                valid_to: None,
                internal_layer: Some(MemoryLayer::Belief),
                tags: vec!["synthesized".to_string(), "profile".to_string()],
                metadata,
                is_retraction: false,
                thread_id: None,
                parent_memory_id: None,
            };

            store.put_memory(&profile_mem)?;
            result.created_memory_ids.push(memory_id);
            result.profiles_updated += 1;
        }

        Ok(())
    }

    // ------------------------------------------------------------------
    // mine_procedures — detect step sequences from episode-layer memories
    // ------------------------------------------------------------------

    fn mine_procedures<S: MemoryStore>(
        &self,
        store: &mut S,
        clock: &dyn Clock,
        result: &mut SynthesisResult,
    ) -> Result<()> {
        let episodes = store.list_memories_by_layer(MemoryLayer::Episode)?;

        // Collect episodes tagged as "procedure_step"
        let steps: Vec<&DurableMemory> = episodes
            .iter()
            .filter(|e| e.tags.iter().any(|t| t == "procedure_step"))
            .collect();

        // Group by entity to find multi-step sequences
        let mut by_entity: BTreeMap<String, Vec<&DurableMemory>> = BTreeMap::new();
        for step in &steps {
            by_entity.entry(step.entity.clone()).or_default().push(step);
        }

        for (entity, seq) in &by_entity {
            if seq.len() < 2 {
                continue;
            }

            let procedure_text = seq
                .iter()
                .enumerate()
                .map(|(i, s)| format!("step{}: {}", i + 1, s.value))
                .collect::<Vec<_>>()
                .join(" → ");

            let proc_id = concept_id_for(entity, &procedure_text);

            // Idempotency check
            let existing = store.list_memories_by_layer(MemoryLayer::Belief)?;
            let already_exists = existing
                .iter()
                .any(|m| m.metadata.get("procedure_id").map(String::as_str) == Some(&proc_id));
            if already_exists {
                continue;
            }

            let memory_id = Uuid::new_v4().to_string();
            let mut metadata = BTreeMap::new();
            metadata.insert("procedure_id".to_string(), proc_id);
            metadata.insert("synthesis_mode".to_string(), "procedure".to_string());
            metadata.insert("step_count".to_string(), seq.len().to_string());

            let proc_mem = DurableMemory {
                memory_id: memory_id.clone(),
                candidate_id: Uuid::new_v4().to_string(),
                stored_at: clock.now(),
                updated_at: None,
                entity: entity.clone(),
                slot: "procedure".to_string(),
                value: procedure_text.clone(),
                raw_text: procedure_text,
                memory_type: MemoryType::Skill,
                confidence: 0.7,
                salience: 0.75,
                scope: Scope::Private,
                ttl: None,
                source: Provenance {
                    source_type: SourceType::System,
                    source_id: "concept_synthesizer".to_string(),
                    source_label: Some("procedure_mine".to_string()),
                    observed_by: None,
                    trust_weight: 0.7,
                },
                event_at: None,
                valid_from: None,
                valid_to: None,
                internal_layer: Some(MemoryLayer::Belief),
                tags: vec![
                    "synthesized".to_string(),
                    "procedure".to_string(),
                    "mined".to_string(),
                ],
                metadata,
                is_retraction: false,
                thread_id: None,
                parent_memory_id: None,
            };

            store.put_memory(&proc_mem)?;
            result.created_memory_ids.push(memory_id);
            result.procedures_mined += 1;
        }

        Ok(())
    }

    // ------------------------------------------------------------------
    // mine_patterns — detect recurring (slot, value) pairs as stable facts
    // ------------------------------------------------------------------

    fn mine_patterns<S: MemoryStore>(
        &self,
        store: &mut S,
        clock: &dyn Clock,
        result: &mut SynthesisResult,
    ) -> Result<()> {
        let episodes = store.list_memories_by_layer(MemoryLayer::Episode)?;

        // Count occurrences of (entity, slot, value_lower)
        let mut freq: BTreeMap<(String, String, String), usize> = BTreeMap::new();
        for ep in &episodes {
            let key = (ep.entity.clone(), ep.slot.clone(), ep.value.to_lowercase());
            *freq.entry(key).or_insert(0) += 1;
        }

        for ((entity, slot, value), count) in &freq {
            if *count < 3 {
                continue;
            }

            let pattern_text = format!("{slot}={value}");
            let pattern_id = concept_id_for(entity, &pattern_text);

            // Idempotency check
            let existing = store.list_memories_by_layer(MemoryLayer::Belief)?;
            let already_exists = existing
                .iter()
                .any(|m| m.metadata.get("pattern_id").map(String::as_str) == Some(&pattern_id));
            if already_exists {
                continue;
            }

            let confidence = (*count as f32 / 10.0).min(1.0);
            let memory_id = Uuid::new_v4().to_string();
            let mut metadata = BTreeMap::new();
            metadata.insert("pattern_id".to_string(), pattern_id);
            metadata.insert("synthesis_mode".to_string(), "pattern".to_string());
            metadata.insert("occurrence_count".to_string(), count.to_string());

            let pattern_mem = DurableMemory {
                memory_id: memory_id.clone(),
                candidate_id: Uuid::new_v4().to_string(),
                stored_at: clock.now(),
                updated_at: None,
                entity: entity.clone(),
                slot: slot.clone(),
                value: value.clone(),
                raw_text: pattern_text,
                memory_type: MemoryType::Fact,
                confidence,
                salience: confidence,
                scope: Scope::Private,
                ttl: None,
                source: Provenance {
                    source_type: SourceType::System,
                    source_id: "concept_synthesizer".to_string(),
                    source_label: Some("pattern_mine".to_string()),
                    observed_by: None,
                    trust_weight: confidence,
                },
                event_at: None,
                valid_from: None,
                valid_to: None,
                internal_layer: Some(MemoryLayer::Belief),
                tags: vec!["synthesized".to_string(), "pattern".to_string()],
                metadata,
                is_retraction: false,
                thread_id: None,
                parent_memory_id: None,
            };

            store.put_memory(&pattern_mem)?;
            result.created_memory_ids.push(memory_id);
            result.patterns_found += 1;
        }

        Ok(())
    }

    // ------------------------------------------------------------------
    // summarize_project — produce a Belief-layer project summary
    // ------------------------------------------------------------------

    fn summarize_project<S: MemoryStore>(
        &self,
        store: &mut S,
        clock: &dyn Clock,
        result: &mut SynthesisResult,
    ) -> Result<()> {
        let all_layers = [MemoryLayer::Episode, MemoryLayer::Belief];
        let mut project_memories: BTreeMap<String, Vec<DurableMemory>> = BTreeMap::new();

        for layer in all_layers {
            for mem in store.list_memories_by_layer(layer)? {
                if let Some(project_id) = mem.metadata.get("ns_project_id").cloned() {
                    project_memories.entry(project_id).or_default().push(mem);
                }
            }
        }

        for (project_id, mems) in &project_memories {
            if mems.len() < 2 {
                continue;
            }

            // Build a terse summary of top-salience facts
            let mut sorted = mems.clone();
            sorted.sort_by(|a, b| {
                b.salience
                    .partial_cmp(&a.salience)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let summary_text = sorted
                .iter()
                .take(5)
                .map(|m| format!("[{}] {}={}", m.entity, m.slot, m.value))
                .collect::<Vec<_>>()
                .join("; ");

            let summary_id = concept_id_for(project_id, &summary_text);

            // Idempotency check
            let existing = store.list_memories_by_layer(MemoryLayer::Belief)?;
            let already_exists = existing.iter().any(|m| {
                m.metadata.get("project_summary_id").map(String::as_str) == Some(&summary_id)
            });
            if already_exists {
                continue;
            }

            let memory_id = Uuid::new_v4().to_string();
            let mut metadata = BTreeMap::new();
            metadata.insert("project_summary_id".to_string(), summary_id);
            metadata.insert("synthesis_mode".to_string(), "project_summary".to_string());
            metadata.insert("ns_project_id".to_string(), project_id.clone());
            metadata.insert("source_count".to_string(), mems.len().to_string());

            let summary_mem = DurableMemory {
                memory_id: memory_id.clone(),
                candidate_id: Uuid::new_v4().to_string(),
                stored_at: clock.now(),
                updated_at: None,
                entity: format!("project:{project_id}"),
                slot: "summary".to_string(),
                value: summary_text.clone(),
                raw_text: summary_text,
                memory_type: MemoryType::Fact,
                confidence: 0.8,
                salience: 0.9,
                scope: Scope::Private,
                ttl: None,
                source: Provenance {
                    source_type: SourceType::System,
                    source_id: "concept_synthesizer".to_string(),
                    source_label: Some("project_summary".to_string()),
                    observed_by: None,
                    trust_weight: 0.8,
                },
                event_at: None,
                valid_from: None,
                valid_to: None,
                internal_layer: Some(MemoryLayer::Belief),
                tags: vec!["synthesized".to_string(), "project_summary".to_string()],
                metadata,
                is_retraction: false,
                thread_id: None,
                parent_memory_id: None,
            };

            store.put_memory(&summary_mem)?;
            result.created_memory_ids.push(memory_id);
            result.project_summaries += 1;
        }

        Ok(())
    }
}

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

/// Derive a stable 8-character concept ID from entity and text.
fn concept_id_for(entity: &str, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(entity.as_bytes());
    hasher.update(b"|");
    hasher.update(text.as_bytes());
    let hash = hasher.finalize();
    format!("{:x}", hash)[..8].to_string()
}

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_memory::adapters::memvid_store::InMemoryMemoryStore;
    use crate::agent_memory::clock::FixedClock;
    use crate::agent_memory::enums::SourceType;
    use crate::agent_memory::ontology::OntologyRegistry;
    use chrono::TimeZone;

    fn clock() -> FixedClock {
        FixedClock::new(chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap())
    }

    fn make_belief(
        id: &str,
        entity: &str,
        slot: &str,
        value: &str,
        confidence: f32,
    ) -> DurableMemory {
        DurableMemory {
            memory_id: id.to_string(),
            candidate_id: Uuid::new_v4().to_string(),
            stored_at: chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            updated_at: None,
            entity: entity.to_string(),
            slot: slot.to_string(),
            value: value.to_string(),
            raw_text: value.to_string(),
            memory_type: MemoryType::Fact,
            confidence,
            salience: 0.8,
            scope: Scope::Private,
            ttl: None,
            source: Provenance {
                source_type: SourceType::External,
                source_id: "test".to_string(),
                source_label: None,
                observed_by: None,
                trust_weight: confidence,
            },
            event_at: None,
            valid_from: None,
            valid_to: None,
            internal_layer: Some(MemoryLayer::Belief),
            tags: Vec::new(),
            metadata: BTreeMap::new(),
            is_retraction: false,
            thread_id: None,
            parent_memory_id: None,
        }
    }

    fn make_episode(id: &str, entity: &str, slot: &str, value: &str) -> DurableMemory {
        let mut mem = make_belief(id, entity, slot, value, 0.7);
        mem.memory_type = MemoryType::Episode;
        mem.internal_layer = Some(MemoryLayer::Episode);
        mem
    }

    /// Seed enough beliefs for a single entity to trigger concept clustering.
    #[test]
    fn test_cluster_creates_concept_node() {
        let mut store = InMemoryMemoryStore::default();
        let clk = clock();

        store
            .put_memory(&make_belief("b1", "alice", "language", "Rust", 0.9))
            .unwrap();
        store
            .put_memory(&make_belief("b2", "alice", "role", "engineer", 0.85))
            .unwrap();

        let synth = ConceptSynthesizer;
        let result = synth
            .synthesize(&mut store, &clk, &mut OntologyRegistry::new(), None)
            .unwrap();

        assert!(
            result.concepts_created >= 1,
            "Expected at least 1 concept node, got {}",
            result.concepts_created
        );
        // Verify concept memory was persisted
        let beliefs = store.list_memories_by_layer(MemoryLayer::Belief).unwrap();
        let concept = beliefs
            .iter()
            .find(|m| m.slot == "concept" && m.entity == "alice");
        assert!(concept.is_some(), "Concept node for alice should be stored");
    }

    /// Running synthesis twice must not create duplicate concept nodes.
    #[test]
    fn test_synthesis_is_idempotent() {
        let mut store = InMemoryMemoryStore::default();
        let clk = clock();

        store
            .put_memory(&make_belief("b1", "bob", "language", "Python", 0.9))
            .unwrap();
        store
            .put_memory(&make_belief("b2", "bob", "framework", "Django", 0.8))
            .unwrap();

        let synth = ConceptSynthesizer;
        let r1 = synth
            .synthesize(&mut store, &clk, &mut OntologyRegistry::new(), None)
            .unwrap();
        let r2 = synth
            .synthesize(&mut store, &clk, &mut OntologyRegistry::new(), None)
            .unwrap();

        assert_eq!(
            r2.concepts_created, 0,
            "Second synthesis pass must not create duplicate nodes (first: {})",
            r1.concepts_created
        );
    }

    /// Episode memories with "procedure_step" tag should be mined into a
    /// procedure belief.
    #[test]
    fn test_procedure_mining() {
        let mut store = InMemoryMemoryStore::default();
        let clk = clock();

        let mut s1 = make_episode("e1", "deploy_proc", "step1", "build image");
        s1.tags.push("procedure_step".to_string());
        let mut s2 = make_episode("e2", "deploy_proc", "step2", "push to registry");
        s2.tags.push("procedure_step".to_string());
        let mut s3 = make_episode("e3", "deploy_proc", "step3", "update deployment");
        s3.tags.push("procedure_step".to_string());

        store.put_memory(&s1).unwrap();
        store.put_memory(&s2).unwrap();
        store.put_memory(&s3).unwrap();

        let synth = ConceptSynthesizer;
        let result = synth
            .synthesize(&mut store, &clk, &mut OntologyRegistry::new(), None)
            .unwrap();

        assert!(
            result.procedures_mined >= 1,
            "Expected at least 1 mined procedure, got {}",
            result.procedures_mined
        );
    }
}
