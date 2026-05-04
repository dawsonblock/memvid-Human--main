//! Ontology registry — deduplication, alias resolution, concept versioning.
//!
//! The [`OntologyRegistry`] is the single source of truth for concept
//! identity. Before any [`ConceptNode`](super::concept_synthesis::ConceptNode)
//! is written to the store the synthesiser must:
//!
//! 1. Call [`ConceptCanonicalizer::normalize`] on the concept text.
//! 2. Call [`OntologyRegistry::resolve_alias`] to check whether the canonical
//!    form already maps to an existing entry.
//! 3. If a match is found, update evidence on the existing entry rather than
//!    creating a duplicate.
//! 4. If no match is found, call [`OntologyRegistry::register`] to record the
//!    new entry and its canonical ID.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

// ── Merge decision ────────────────────────────────────────────────────────────

/// Governance decision for two concept entries that appear to overlap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConceptMergeDecision {
    /// Collapse into a single entry, keeping `source_ids[0]` as canonical.
    Merge,
    /// The two concepts are genuinely distinct; keep both.
    PreserveSeparate,
    /// One concept supersedes the other; superseded entry is retired.
    Supersede,
    /// One ID is just another spelling of the other; add it as an alias.
    Alias,
    /// The concept is too broad and should be split into finer entries.
    Split,
    /// Merge is invalid (e.g. cycle, wrong entity) — do nothing.
    Reject,
}

// ── Merge record ─────────────────────────────────────────────────────────────

/// Audit entry for a completed (or attempted) concept merge.
#[derive(Debug, Clone)]
pub struct ConceptMergeRecord {
    pub merge_id: String,
    /// Concept IDs that were involved (typically two).
    pub source_ids: Vec<String>,
    pub decision: ConceptMergeDecision,
    pub rationale: String,
    pub merged_at: DateTime<Utc>,
}

// ── Version entry ─────────────────────────────────────────────────────────────

/// One point in the confidence history of a concept entry.
#[derive(Debug, Clone)]
pub struct ConceptVersionEntry {
    pub concept_id: String,
    pub confidence: f32,
    pub change_reason: String,
    pub recorded_at: DateTime<Utc>,
}

// ── Version history (bounded) ─────────────────────────────────────────────────

const MAX_VERSION_HISTORY: usize = 50;

/// Bounded chronological confidence history for a concept.
#[derive(Debug, Clone, Default)]
pub struct ConceptVersionHistory {
    entries: Vec<ConceptVersionEntry>,
}

impl ConceptVersionHistory {
    pub fn push(&mut self, entry: ConceptVersionEntry) {
        if self.entries.len() >= MAX_VERSION_HISTORY {
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }

    pub fn entries(&self) -> &[ConceptVersionEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ── Ontology entry ────────────────────────────────────────────────────────────

/// A single canonicalised concept living in the registry.
#[derive(Debug, Clone)]
pub struct OntologyEntry {
    /// Stable ID for this concept (same format as ConceptSynthesizer's concept_id).
    pub canonical_id: String,
    /// Canonical (normalised) text representation of the concept.
    pub canonical_text: String,
    /// The entity this concept describes (e.g. "alice", "project_x").
    pub entity: String,
    /// Current confidence estimate derived from supporting beliefs.
    pub confidence: f32,
    /// Alternative text spellings that resolve to this canonical entry.
    pub aliases: Vec<String>,
    /// IDs of belief memories that provide evidence for this concept.
    pub supporting_memory_ids: Vec<String>,
    /// Bounded confidence / reason history.
    pub version_history: ConceptVersionHistory,
    /// Audit log of merges that involved this entry.
    pub merge_history: Vec<ConceptMergeRecord>,
    /// If this entry was retired in favour of another, the winning concept ID.
    pub superseded_by: Option<String>,
    /// Unix-epoch timestamp (seconds) when this entry was first registered.
    pub created_at: i64,
    /// Unix-epoch timestamp (seconds) of the most recent update.
    pub updated_at: i64,
}

impl OntologyEntry {
    /// Returns `true` when the entry has been superseded and should not be
    /// returned as a primary result.
    pub fn is_retired(&self) -> bool {
        self.superseded_by.is_some()
    }
}

// ── Concept canonicalizer ─────────────────────────────────────────────────────

/// Converts raw concept text into a stable, normalised canonical form.
///
/// The canonical form is: trim → lower-case → split on whitespace →
/// deduplicate tokens → sort tokens → rejoin with spaces.
/// This makes "Python Programming" and "programming Python" map to the
/// same form.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConceptCanonicalizer;

impl ConceptCanonicalizer {
    /// Normalise `text` into a canonical token-sorted form.
    pub fn normalize(&self, text: &str) -> String {
        let mut tokens: Vec<String> = text
            .split_whitespace()
            .map(|t| {
                t.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .filter(|t| !t.is_empty())
            .collect();
        tokens.sort_unstable();
        tokens.dedup();
        tokens.join(" ")
    }

    /// Return `true` when `a` and `b` normalise to the same form.
    pub fn are_equivalent(&self, a: &str, b: &str) -> bool {
        self.normalize(a) == self.normalize(b)
    }
}

// ── Semantic drift tracker ────────────────────────────────────────────────────

/// Detects when a concept's confidence has shifted enough to warrant a
/// version-history record.
#[derive(Debug, Clone, Copy)]
pub struct SemanticDriftTracker {
    /// Minimum absolute confidence change before drift is recorded.
    pub threshold: f32,
}

impl Default for SemanticDriftTracker {
    fn default() -> Self {
        Self { threshold: 0.1 }
    }
}

impl SemanticDriftTracker {
    pub fn new(threshold: f32) -> Self {
        Self { threshold }
    }

    /// Returns `true` when `new_confidence` diverges from `entry.confidence`
    /// by at least `self.threshold`.
    pub fn check_drift(&self, entry: &OntologyEntry, new_confidence: f32) -> bool {
        (entry.confidence - new_confidence).abs() >= self.threshold
    }
}

// ── Ontology registry ─────────────────────────────────────────────────────────

/// Central registry of all canonicalised concept identities.
///
/// `entries` is keyed by `canonical_id`.
/// `alias_index` maps every known alias (including canonical forms) to the
/// `canonical_id` that owns them, enabling O(log n) alias look-up.
#[derive(Debug, Default, Clone)]
pub struct OntologyRegistry {
    entries: BTreeMap<String, OntologyEntry>,
    /// Maps normalised alias text → canonical_id.
    alias_index: BTreeMap<String, String>,
    canonicalizer: ConceptCanonicalizer,
    drift_tracker: SemanticDriftTracker,
}

impl OntologyRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Registration ─────────────────────────────────────────────────────────

    /// Register a new concept entry.
    ///
    /// If the canonical form of `canonical_text` is already mapped via the
    /// alias index, this call is a no-op and returns the existing
    /// `canonical_id`.  Otherwise the new entry is stored and indexed.
    pub fn register(
        &mut self,
        canonical_id: impl Into<String>,
        canonical_text: impl Into<String>,
        entity: impl Into<String>,
        confidence: f32,
        now_ts: i64,
    ) -> String {
        let canonical_text = canonical_text.into();
        let normalised = self.canonicalizer.normalize(&canonical_text);

        // Return the existing canonical_id if already registered.
        if let Some(existing_id) = self.alias_index.get(&normalised) {
            return existing_id.clone();
        }

        let cid = canonical_id.into();
        let entry = OntologyEntry {
            canonical_id: cid.clone(),
            canonical_text: canonical_text.clone(),
            entity: entity.into(),
            confidence,
            aliases: vec![normalised.clone()],
            supporting_memory_ids: Vec::new(),
            version_history: ConceptVersionHistory::default(),
            merge_history: Vec::new(),
            superseded_by: None,
            created_at: now_ts,
            updated_at: now_ts,
        };

        self.alias_index.insert(normalised, cid.clone());
        self.entries.insert(cid.clone(), entry);
        cid
    }

    // ── Lookup ────────────────────────────────────────────────────────────────

    /// Resolve an alias or canonical text to the owning entry's
    /// `canonical_id`, if registered.
    pub fn resolve_alias(&self, text: &str) -> Option<&str> {
        let normalised = self.canonicalizer.normalize(text);
        self.alias_index.get(&normalised).map(String::as_str)
    }

    /// Get a reference to an entry by its canonical ID.
    pub fn get(&self, canonical_id: &str) -> Option<&OntologyEntry> {
        self.entries.get(canonical_id)
    }

    /// Get a mutable reference to an entry by its canonical ID.
    pub fn get_mut(&mut self, canonical_id: &str) -> Option<&mut OntologyEntry> {
        self.entries.get_mut(canonical_id)
    }

    /// Collect all non-retired entries for a given entity.
    pub fn find_by_entity(&self, entity: &str) -> Vec<&OntologyEntry> {
        self.entries
            .values()
            .filter(|e| e.entity == entity && !e.is_retired())
            .collect()
    }

    /// Iterate over all entries (including retired ones).
    pub fn all_entries(&self) -> impl Iterator<Item = &OntologyEntry> {
        self.entries.values()
    }

    // ── Alias management ──────────────────────────────────────────────────────

    /// Add `alias_text` as another name for `canonical_id`.
    ///
    /// Returns `false` if `canonical_id` does not exist or if the alias is
    /// already mapped to a *different* canonical ID.
    pub fn add_alias(&mut self, canonical_id: &str, alias_text: &str, now_ts: i64) -> bool {
        if !self.entries.contains_key(canonical_id) {
            return false;
        }
        let normalised = self.canonicalizer.normalize(alias_text);
        // Accept only if unregistered or already points to the same entry.
        match self.alias_index.get(&normalised) {
            Some(existing_cid) if existing_cid != canonical_id => return false,
            _ => {}
        }
        self.alias_index
            .entry(normalised.clone())
            .or_insert_with(|| canonical_id.to_string());
        if let Some(entry) = self.entries.get_mut(canonical_id) {
            if !entry.aliases.contains(&normalised) {
                entry.aliases.push(normalised);
            }
            entry.updated_at = now_ts;
        }
        true
    }

    // ── Evidence update ───────────────────────────────────────────────────────

    /// Append a supporting memory ID to an existing entry and optionally
    /// update its confidence (recording drift if threshold is exceeded).
    pub fn add_supporting_memory(
        &mut self,
        canonical_id: &str,
        memory_id: impl Into<String>,
        new_confidence: Option<f32>,
        change_reason: impl Into<String>,
        now_ts: i64,
    ) {
        if let Some(entry) = self.entries.get_mut(canonical_id) {
            let mid = memory_id.into();
            if !entry.supporting_memory_ids.contains(&mid) {
                entry.supporting_memory_ids.push(mid);
            }
            if let Some(nc) = new_confidence {
                if self.drift_tracker.check_drift(entry, nc) {
                    entry.version_history.push(ConceptVersionEntry {
                        concept_id: canonical_id.to_string(),
                        confidence: entry.confidence,
                        change_reason: change_reason.into(),
                        recorded_at: DateTime::<Utc>::from_timestamp(now_ts, 0)
                            .unwrap_or(Utc::now()),
                    });
                }
                entry.confidence = nc;
            }
            entry.updated_at = now_ts;
        }
    }

    // ── Merge / retire ────────────────────────────────────────────────────────

    /// Propose a merge of `id_a` into `id_b` (id_b remains canonical).
    ///
    /// On `ConceptMergeDecision::Merge`:
    /// - All aliases from `id_a` are moved to `id_b`.
    /// - `id_a.superseded_by` is set to `id_b`.
    /// - A merge record is appended to both entries' merge histories.
    ///
    /// Returns `Err` as a string description if the merge would:
    /// - create a self-loop (`id_a == id_b`)
    /// - involve an already-retired entry
    pub fn propose_merge(
        &mut self,
        id_a: &str,
        id_b: &str,
        decision: ConceptMergeDecision,
        rationale: impl Into<String>,
        merge_id: impl Into<String>,
        now_ts: i64,
    ) -> Result<(), String> {
        if id_a == id_b {
            return Err("cannot merge a concept with itself".to_string());
        }
        if !self.entries.contains_key(id_a) {
            return Err(format!("concept '{id_a}' not found"));
        }
        if !self.entries.contains_key(id_b) {
            return Err(format!("concept '{id_b}' not found"));
        }
        if self.entries[id_a].superseded_by.is_some() {
            return Err(format!("concept '{id_a}' is already retired"));
        }
        if self.entries[id_b].superseded_by.is_some() {
            return Err(format!("concept '{id_b}' is already retired"));
        }

        let rationale = rationale.into();
        let merge_id = merge_id.into();

        let record_a = ConceptMergeRecord {
            merge_id: merge_id.clone(),
            source_ids: vec![id_a.to_string(), id_b.to_string()],
            decision: decision.clone(),
            rationale: rationale.clone(),
            merged_at: DateTime::<Utc>::from_timestamp(now_ts, 0).unwrap_or(Utc::now()),
        };
        let record_b = record_a.clone();

        if matches!(
            decision,
            ConceptMergeDecision::Merge | ConceptMergeDecision::Supersede
        ) {
            // Move aliases from id_a to id_b
            let aliases_a: Vec<String> = self.entries[id_a].aliases.clone();
            for alias in &aliases_a {
                self.alias_index.insert(alias.clone(), id_b.to_string());
            }
            if let Some(entry_b) = self.entries.get_mut(id_b) {
                for alias in &aliases_a {
                    if !entry_b.aliases.contains(alias) {
                        entry_b.aliases.push(alias.clone());
                    }
                }
                entry_b.merge_history.push(record_b);
                entry_b.updated_at = now_ts;
            }
            // Retire id_a
            if let Some(entry_a) = self.entries.get_mut(id_a) {
                entry_a.superseded_by = Some(id_b.to_string());
                entry_a.merge_history.push(record_a);
                entry_a.updated_at = now_ts;
            }
        } else {
            // Non-destructive: just record the audit trail on both
            if let Some(e) = self.entries.get_mut(id_a) {
                e.merge_history.push(record_a);
                e.updated_at = now_ts;
            }
            if let Some(e) = self.entries.get_mut(id_b) {
                e.merge_history.push(record_b);
                e.updated_at = now_ts;
            }
        }

        Ok(())
    }

    /// Retire an entry by marking it as superseded by `successor_id`.
    /// All aliases are re-pointed to `successor_id`.
    pub fn retire(
        &mut self,
        canonical_id: &str,
        successor_id: &str,
        now_ts: i64,
    ) -> Result<(), String> {
        if canonical_id == successor_id {
            return Err("cannot retire in favour of itself".to_string());
        }
        if !self.entries.contains_key(successor_id) {
            return Err(format!("successor '{successor_id}' not found"));
        }
        let aliases: Vec<String> = self
            .entries
            .get(canonical_id)
            .map(|e| e.aliases.clone())
            .unwrap_or_default();
        for alias in &aliases {
            self.alias_index
                .insert(alias.clone(), successor_id.to_string());
        }
        if let Some(entry) = self.entries.get_mut(canonical_id) {
            entry.superseded_by = Some(successor_id.to_string());
            entry.updated_at = now_ts;
        }
        if let Some(entry) = self.entries.get_mut(successor_id) {
            for alias in &aliases {
                if !entry.aliases.contains(alias) {
                    entry.aliases.push(alias.clone());
                }
            }
            entry.updated_at = now_ts;
        }
        Ok(())
    }

    // ── Drift query ───────────────────────────────────────────────────────────

    /// Returns all entries whose confidence changed materially (by at least
    /// `threshold`) relative to the oldest version-history record, since
    /// `since_ts`.
    pub fn drift_since(&self, since_ts: i64, threshold: f32) -> Vec<(&OntologyEntry, f32)> {
        let mut out = Vec::new();
        for entry in self.entries.values() {
            if entry.is_retired() {
                continue;
            }
            let Some(oldest_in_window) = entry
                .version_history
                .entries()
                .iter()
                .find(|v| v.recorded_at.timestamp() >= since_ts)
            else {
                continue;
            };
            let delta = (oldest_in_window.confidence - entry.confidence).abs();
            if delta >= threshold {
                out.push((entry, delta));
            }
        }
        out
    }

    /// Returns the total number of entries (including retired).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizer_is_order_and_case_insensitive() {
        let c = ConceptCanonicalizer;
        assert_eq!(
            c.normalize("Python Programming"),
            c.normalize("programming python")
        );
        assert_eq!(c.normalize("  Rust  Lang  "), c.normalize("lang rust"));
    }

    #[test]
    fn registry_deduplicates_on_canonical_form() {
        let mut reg = OntologyRegistry::new();
        let id1 = reg.register("cid-1", "Python Programming", "lang", 0.9, 1000);
        let id2 = reg.register("cid-2", "programming Python", "lang", 0.8, 1001);
        assert_eq!(id1, id2, "alias-equivalent texts must map to same entry");
        assert_eq!(reg.len(), 1);
    }
}
