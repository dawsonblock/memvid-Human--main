//! Graph Governance — edge lifecycle, cycle prevention, entropy monitoring,
//! and traversal noise suppression for the agent memory knowledge graph.
//!
//! The [`GraphEdgeStore`] is the single governed authority for knowledge-graph
//! edges. All writes pass through `add_edge`, which enforces:
//!
//! 1. **Confidence gate** — edges below `min_confidence` are silently rejected.
//! 2. **Fan-out cap** — at most `max_edges_per_node` outgoing edges per node.
//! 3. **Cycle prevention** — [`CycleLimiter`] performs a DFS up to
//!    `max_traversal_depth` hops before accepting each new edge.

use std::collections::{BTreeMap, BTreeSet};

use uuid::Uuid;

// ── EdgeKind ─────────────────────────────────────────────────────────────────

/// Semantic classification of a directed knowledge-graph edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeKind {
    /// The source concept is loosely associated with the target.
    RelatedTo,
    /// The source supersedes (replaces) the target.
    Supersedes,
    /// The source was derived from the target (e.g., a concept from beliefs).
    DerivedFrom,
    /// The source contradicts the target.
    Contradicts,
    /// The source supports (corroborates) the target.
    Supports,
}

// ── GraphEdge ─────────────────────────────────────────────────────────────────

/// A single directed edge in the knowledge graph.
#[derive(Debug, Clone)]
pub struct GraphEdge {
    /// Stable UUID for this edge.
    pub id: String,
    /// ID of the source node (concept / memory).
    pub source_id: String,
    /// ID of the target node (concept / memory).
    pub target_id: String,
    /// Semantic type of the relationship.
    pub kind: EdgeKind,
    /// How confident we are that this edge is correct \[0, 1\].
    pub confidence: f32,
    /// The memory that originally triggered creation of this edge (if any).
    pub origin_memory_id: Option<String>,
    /// Current strength of the edge (decays over time).
    pub strength: f32,
    /// Fraction of `strength` lost per day of elapsed time.
    pub decay_per_day: f32,
    /// Unix timestamp of the last reinforcement event.
    pub last_reinforced_at: i64,
    /// Unix timestamp of edge creation.
    pub created_at: i64,
}

impl GraphEdge {
    /// Convenience constructor with sensible defaults for strength / decay.
    pub fn new(
        source_id: impl Into<String>,
        target_id: impl Into<String>,
        kind: EdgeKind,
        confidence: f32,
        origin_memory_id: Option<String>,
        created_at: i64,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            source_id: source_id.into(),
            target_id: target_id.into(),
            kind,
            confidence,
            origin_memory_id,
            strength: confidence,
            decay_per_day: 0.01,
            last_reinforced_at: created_at,
            created_at,
        }
    }
}

// ── GraphGovernancePolicy ────────────────────────────────────────────────────

/// Policy parameters for the knowledge-graph governor.
#[derive(Debug, Clone)]
pub struct GraphGovernancePolicy {
    /// Maximum number of outgoing edges allowed from a single node.
    pub max_edges_per_node: usize,
    /// Edges (or residual strength) below this threshold are rejected / pruned.
    pub min_confidence: f32,
    /// Maximum DFS depth when checking for cycles.
    pub max_traversal_depth: usize,
}

impl Default for GraphGovernancePolicy {
    fn default() -> Self {
        Self {
            max_edges_per_node: 50,
            min_confidence: 0.2,
            max_traversal_depth: 8,
        }
    }
}

// ── CycleLimiter ─────────────────────────────────────────────────────────────

/// Stateless cycle detector.
///
/// Uses depth-first search over the *existing* outgoing edges of the store to
/// determine whether adding a candidate edge would introduce a cycle.
pub struct CycleLimiter;

impl CycleLimiter {
    /// Returns `true` if adding `source_id → target_id` would create a cycle.
    ///
    /// A cycle exists when `source_id` is reachable from `target_id` through
    /// the current graph edges within `max_depth` hops.
    pub fn would_create_cycle(
        store: &GraphEdgeStore,
        source_id: &str,
        target_id: &str,
        max_depth: usize,
    ) -> bool {
        // Trivial self-loop
        if source_id == target_id {
            return true;
        }
        // DFS from target_id: can we reach source_id?
        let mut stack: Vec<(String, usize)> = vec![(target_id.to_string(), 0)];
        let mut visited: BTreeSet<String> = BTreeSet::new();

        while let Some((node, depth)) = stack.pop() {
            if depth > max_depth {
                continue;
            }
            if node == source_id {
                return true;
            }
            if !visited.insert(node.clone()) {
                continue;
            }
            if let Some(edge_ids) = store.outgoing.get(&node) {
                for edge_id in edge_ids {
                    if let Some(edge) = store.edges.get(edge_id) {
                        stack.push((edge.target_id.clone(), depth + 1));
                    }
                }
            }
        }
        false
    }
}

// ── GraphEntropyMonitor ───────────────────────────────────────────────────────

/// Tracks the ratio of edges added to edges removed by decay.
///
/// A high ratio indicates runaway graph expansion: the graph is growing much
/// faster than it is being pruned.
#[derive(Debug, Clone, Default)]
pub struct GraphEntropyMonitor {
    /// Total edges successfully inserted since construction.
    pub edges_added: u64,
    /// Total edges removed by `decay_all` since construction.
    pub edges_removed_by_decay: u64,
    /// Ratio above which `is_alert()` returns `true`.
    pub alert_threshold: f32,
}

impl GraphEntropyMonitor {
    /// Construct a monitor with the given alert threshold.
    pub fn new(alert_threshold: f32) -> Self {
        Self {
            alert_threshold,
            ..Default::default()
        }
    }

    /// Returns `edges_added / edges_removed_by_decay`.
    ///
    /// Returns `edges_added` as a float when no edges have been removed yet,
    /// avoiding a division-by-zero.
    pub fn entropy_ratio(&self) -> f32 {
        if self.edges_removed_by_decay == 0 {
            self.edges_added as f32
        } else {
            self.edges_added as f32 / self.edges_removed_by_decay as f32
        }
    }

    /// Returns `true` when the entropy ratio exceeds `alert_threshold`.
    pub fn is_alert(&self) -> bool {
        self.entropy_ratio() > self.alert_threshold
    }
}

// ── TraversalNoiseSuppressor ──────────────────────────────────────────────────

/// Filters low-confidence edges out of a traversal result set.
pub struct TraversalNoiseSuppressor;

impl TraversalNoiseSuppressor {
    /// Return only edges whose `confidence` meets or exceeds `min_confidence`.
    pub fn filter_edges<'a>(edges: Vec<&'a GraphEdge>, min_confidence: f32) -> Vec<&'a GraphEdge> {
        edges
            .into_iter()
            .filter(|e| e.confidence >= min_confidence)
            .collect()
    }
}

// ── EdgeDecay ─────────────────────────────────────────────────────────────────

/// Applies time-based strength decay to a single edge.
pub struct EdgeDecay;

impl EdgeDecay {
    /// Decrease `edge.strength` by `edge.decay_per_day * elapsed_days`.
    pub fn apply(edge: &mut GraphEdge, elapsed_days: f32) {
        edge.strength -= edge.decay_per_day * elapsed_days;
    }
}

// ── GraphEdgeStore ────────────────────────────────────────────────────────────

/// Governed in-memory knowledge-graph edge store.
///
/// All mutations go through [`GraphEdgeStore::add_edge`], which enforces the
/// [`GraphGovernancePolicy`] on every write.  Periodic calls to
/// [`GraphEdgeStore::decay_all`] prune edges whose strength has fallen below
/// the policy's `min_confidence`.
pub struct GraphEdgeStore {
    /// Primary edge table, keyed by edge ID.
    pub(crate) edges: BTreeMap<String, GraphEdge>,
    /// Forward index: source node ID → list of edge IDs.
    pub(crate) outgoing: BTreeMap<String, Vec<String>>,
    /// Reverse index: target node ID → list of edge IDs.
    pub(crate) incoming: BTreeMap<String, Vec<String>>,
    /// Active governance policy.
    policy: GraphGovernancePolicy,
    /// Entropy tracker.
    entropy_monitor: GraphEntropyMonitor,
}

impl GraphEdgeStore {
    /// Create a store with default governance policy.
    pub fn new() -> Self {
        Self::with_policy(GraphGovernancePolicy::default())
    }

    /// Create a store with a custom governance policy.
    pub fn with_policy(policy: GraphGovernancePolicy) -> Self {
        Self {
            edges: BTreeMap::new(),
            outgoing: BTreeMap::new(),
            incoming: BTreeMap::new(),
            entropy_monitor: GraphEntropyMonitor::new(10.0),
            policy,
        }
    }

    /// Read-only access to the current governance policy.
    pub fn policy(&self) -> &GraphGovernancePolicy {
        &self.policy
    }

    /// Number of live edges in the store.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Current entropy ratio (add:decay).
    pub fn entropy_ratio(&self) -> f32 {
        self.entropy_monitor.entropy_ratio()
    }

    /// `true` when entropy exceeds the alert threshold.
    pub fn entropy_alert(&self) -> bool {
        self.entropy_monitor.is_alert()
    }

    /// Attempt to add an edge, applying all governance checks.
    ///
    /// Returns `true` when the edge was inserted, `false` when it was
    /// rejected by the policy (not an error — just a governed no-op).
    pub fn add_edge(&mut self, mut edge: GraphEdge) -> bool {
        // 1. Confidence gate
        if edge.confidence < self.policy.min_confidence {
            return false;
        }

        // 2. Fan-out cap
        let outgoing_count = self.outgoing.get(&edge.source_id).map_or(0, Vec::len);
        if outgoing_count >= self.policy.max_edges_per_node {
            return false;
        }

        // 3. Cycle prevention
        if CycleLimiter::would_create_cycle(
            self,
            &edge.source_id,
            &edge.target_id,
            self.policy.max_traversal_depth,
        ) {
            return false;
        }

        // Assign a stable ID when missing
        if edge.id.is_empty() {
            edge.id = Uuid::new_v4().to_string();
        }

        let edge_id = edge.id.clone();
        let source_id = edge.source_id.clone();
        let target_id = edge.target_id.clone();

        self.outgoing
            .entry(source_id)
            .or_default()
            .push(edge_id.clone());
        self.incoming
            .entry(target_id)
            .or_default()
            .push(edge_id.clone());
        self.edges.insert(edge_id, edge);

        self.entropy_monitor.edges_added += 1;
        true
    }

    /// Remove the edge with the given ID.
    ///
    /// Returns `true` if an edge was found and removed.
    pub fn remove_edge(&mut self, edge_id: &str) -> bool {
        if let Some(edge) = self.edges.remove(edge_id) {
            if let Some(out) = self.outgoing.get_mut(&edge.source_id) {
                out.retain(|id| id != edge_id);
            }
            if let Some(inc) = self.incoming.get_mut(&edge.target_id) {
                inc.retain(|id| id != edge_id);
            }
            true
        } else {
            false
        }
    }

    /// Return all live edges whose source is `source_id`.
    pub fn edges_from(&self, source_id: &str) -> Vec<&GraphEdge> {
        self.outgoing
            .get(source_id)
            .map(|ids| ids.iter().filter_map(|id| self.edges.get(id)).collect())
            .unwrap_or_default()
    }

    /// Return all live edges whose target is `target_id`.
    pub fn edges_to(&self, target_id: &str) -> Vec<&GraphEdge> {
        self.incoming
            .get(target_id)
            .map(|ids| ids.iter().filter_map(|id| self.edges.get(id)).collect())
            .unwrap_or_default()
    }

    /// Apply time-based decay to every edge and prune those that fall below
    /// `min_confidence`.
    ///
    /// `elapsed_days` is the number of days since the last decay pass.
    pub fn decay_all(&mut self, elapsed_days: f32) {
        let min_confidence = self.policy.min_confidence;

        let to_remove: Vec<String> = self
            .edges
            .iter_mut()
            .filter_map(|(id, edge)| {
                EdgeDecay::apply(edge, elapsed_days);
                if edge.strength < min_confidence {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();

        let removed_count = to_remove.len() as u64;
        for edge_id in to_remove {
            self.remove_edge(&edge_id);
        }

        self.entropy_monitor.edges_removed_by_decay += removed_count;
    }
}

impl Default for GraphEdgeStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_edge(src: &str, tgt: &str, confidence: f32) -> GraphEdge {
        GraphEdge::new(src, tgt, EdgeKind::RelatedTo, confidence, None, 0)
    }

    #[test]
    fn add_and_retrieve_edge() {
        let mut store = GraphEdgeStore::new();
        let edge = make_edge("A", "B", 0.9);
        assert!(store.add_edge(edge));
        assert_eq!(store.edges_from("A").len(), 1);
        assert_eq!(store.edges_to("B").len(), 1);
        assert_eq!(store.edge_count(), 1);
    }

    #[test]
    fn remove_edge_cleans_indexes() {
        let mut store = GraphEdgeStore::new();
        let edge = make_edge("A", "B", 0.9);
        let id = edge.id.clone();
        store.add_edge(edge);
        assert!(store.remove_edge(&id));
        assert!(store.edges_from("A").is_empty());
        assert!(store.edges_to("B").is_empty());
        assert_eq!(store.edge_count(), 0);
    }
}
