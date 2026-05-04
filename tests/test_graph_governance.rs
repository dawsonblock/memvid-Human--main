use memvid::agent_memory::graph::{
    EdgeKind, GraphEdge, GraphEdgeStore, GraphGovernancePolicy, TraversalNoiseSuppressor,
};

fn edge(src: &str, tgt: &str, kind: EdgeKind, confidence: f32) -> GraphEdge {
    GraphEdge::new(src.to_string(), tgt.to_string(), kind, confidence, None, 0)
}

/// The fan-out cap in GraphGovernancePolicy.max_edges_per_node must reject
/// the (N+1)th outgoing edge from the same source node.
#[test]
fn runaway_graph_expansion_blocked_by_max_edges_per_node() {
    let policy = GraphGovernancePolicy {
        max_edges_per_node: 3,
        ..GraphGovernancePolicy::default()
    };
    let mut store = GraphEdgeStore::with_policy(policy);

    assert!(store.add_edge(edge("A", "B1", EdgeKind::RelatedTo, 0.9)));
    assert!(store.add_edge(edge("A", "B2", EdgeKind::RelatedTo, 0.9)));
    assert!(store.add_edge(edge("A", "B3", EdgeKind::RelatedTo, 0.9)));

    // 4th outgoing edge from "A" must be rejected.
    let accepted = store.add_edge(edge("A", "B4", EdgeKind::RelatedTo, 0.9));
    assert!(!accepted, "Should reject edge when fan-out cap is reached");
    assert_eq!(store.edge_count(), 3);
}

/// An edge whose confidence falls below min_confidence must be silently
/// rejected by add_edge (returns false, nothing stored).
#[test]
fn noisy_edge_suppressed_below_confidence_threshold() {
    let policy = GraphGovernancePolicy {
        min_confidence: 0.2,
        ..GraphGovernancePolicy::default()
    };
    let mut store = GraphEdgeStore::with_policy(policy);

    let accepted = store.add_edge(edge("X", "Y", EdgeKind::Supports, 0.1));
    assert!(!accepted, "Edge with confidence 0.1 < 0.2 must be rejected");
    assert_eq!(store.edge_count(), 0);
}

/// The DFS cycle guard must prevent an edge that would close a cycle:
/// If A→B and B→C exist, then C→A must be rejected.
#[test]
fn cyclic_association_prevented() {
    let mut store = GraphEdgeStore::default();

    assert!(store.add_edge(edge("A", "B", EdgeKind::RelatedTo, 0.9)));
    assert!(store.add_edge(edge("B", "C", EdgeKind::RelatedTo, 0.9)));

    let accepted = store.add_edge(edge("C", "A", EdgeKind::RelatedTo, 0.9));
    assert!(
        !accepted,
        "C→A would create a cycle A→B→C→A; must be rejected"
    );
    assert_eq!(store.edge_count(), 2);
}

/// After decay_all(days), edge strength must decrease by decay_per_day * days.
/// An edge whose remaining strength is still above min_confidence must NOT be
/// removed.
#[test]
fn edge_decay_reduces_strength_over_time() {
    let mut store = GraphEdgeStore::default();

    let mut e = edge("P", "Q", EdgeKind::DerivedFrom, 1.0);
    e.decay_per_day = 0.1;
    assert!(store.add_edge(e));
    assert_eq!(store.edge_count(), 1);

    store.decay_all(5.0); // strength should drop to 1.0 - 0.5 = 0.5

    assert_eq!(
        store.edge_count(),
        1,
        "Edge with strength 0.5 >= 0.2 min should survive"
    );

    let strength = store
        .edges_from("P")
        .into_iter()
        .next()
        .expect("Edge should still be present")
        .strength;
    let approx = (strength - 0.5_f32).abs();
    assert!(approx < 1e-4, "Expected strength ≈ 0.5, got {}", strength);
}

/// TraversalNoiseSuppressor::filter_edges must exclude edges whose confidence
/// is below the given threshold without touching higher-confidence edges.
#[test]
fn low_confidence_traversal_suppressed() {
    let mut store = GraphEdgeStore::default();

    let high = edge("M", "N1", EdgeKind::Supports, 0.8);
    let low = edge("M", "N2", EdgeKind::Supports, 0.1);
    store.add_edge(high);
    store.add_edge(low);

    let all_edges = store.edges_from("M");
    let filtered = TraversalNoiseSuppressor::filter_edges(all_edges, 0.5);

    assert_eq!(
        filtered.len(),
        1,
        "Only the high-confidence edge should survive filtering"
    );
    assert!(
        filtered[0].confidence >= 0.5,
        "Remaining edge confidence should be >= 0.5"
    );
}
