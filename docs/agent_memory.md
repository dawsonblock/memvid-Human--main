# Agent Memory

`src/agent_memory` is a governed in-process memory layer built on top of the `Memvid` kernel.
It provides a **six-layer bounded memory architecture** with a single controlled entry point
(`MemoryController`) for all reads and writes.

---

## Why a layered model

Raw `Memvid` stores every frame equally — it has no notion of "this is a transient observation"
vs. "this is a long-lived fact". The agent memory subsystem adds:

1. **Semantic routing** — classifying incoming observations into the correct retention layer
2. **Promotion gates** — only high-confidence, high-salience observations graduate to durable storage
3. **Conflict detection** — contradicting beliefs are flagged or retracted automatically
4. **Lifecycle governance** — procedures have explicit status transitions with auditable reasons
5. **Retention policy** — each layer has independent TTL and decay rules
6. **Audit trail** — every classification, promotion, and retrieval is logged to an append-only audit stream

---

## The six layers

| Layer | `MemoryLayer` variant | Typical content | Retention |
|---|---|---|---|
| Trace | `Trace` | Raw chat turns, tool outputs, transient observations | Short (hours–days) |
| Episode | `Episode` | Consolidated event records ("the user deployed alpha on Fri") | Medium (weeks) |
| Belief | `Belief` | Long-lived factual assertions with polarity and confidence | Long (indefinite) |
| GoalState | `GoalState` | Active tasks, objectives, sub-goals and their status | Task lifetime |
| SelfModel | `SelfModel` | Agent capability descriptions, identity, configuration facts | Persistent |
| Procedure | `Procedure` | Learned multi-step processes with explicit lifecycle transitions | Persistent |

Each layer maps to a dedicated store module (`trace_store` is folded into `episode_store` for
compactness; the others have their own files: `belief_store`, `episode_store`, `goal_state_store`,
`self_model_store`, `procedure_store`).

---

## `MemoryController`

The single governed authority for all writes and reads.

```rust
pub struct MemoryController<S: MemoryStore> {
    store: S,
    clock: Arc<dyn Clock>,
    audit: AuditLogger,
    classifier: MemoryClassifier,
    promoter: MemoryPromoter,
    belief_updater: BeliefUpdater,
    retriever: MemoryRetriever,
    consolidation_engine: ConsolidationEngine,
}
```

### Construction

```rust
use memvid_core::agent_memory::MemoryController;

let controller = MemoryController::new(
    store,               // impl MemoryStore
    clock,               // Arc<dyn Clock>
    audit_logger,
    classifier,
    promoter,
    belief_updater,
    retriever,
);
```

### Ingest

```rust
let result = controller.ingest(candidate)?;
// Returns Option<String> — the stable memory_id if the candidate was promoted to durable storage,
// or None if it was rejected or stored only in the Trace layer.
```

### Retrieve

```rust
let query = RetrievalQuery {
    text: "what is the deployment target?".to_string(),
    intent: QueryIntent::CurrentFact,
    top_k: 5,
    ..Default::default()
};
let hits: Vec<RetrievalHit> = controller.retrieve(&query)?;
```

---

## Key types

### `CandidateMemory`

Input to `MemoryController::ingest`. Carries the raw observation plus its provenance.

```rust
pub struct CandidateMemory {
    pub candidate_id: String,
    pub observed_at: DateTime<Utc>,
    pub entity: String,      // Subject of the memory ("user", "system", …)
    pub slot: String,        // Property name ("preferred_language", "deploy_target", …)
    pub value: String,       // Property value
    pub raw_text: String,    // Original text the observation came from
    pub source: Provenance,
    pub memory_type: MemoryType,   // Trace | Episode | Fact | Preference | GoalState
    pub confidence: f32,     // 0.0–1.0
    pub salience: f32,       // 0.0–1.0
    pub scope: Scope,        // Private | Task | Project | Shared
    pub ttl: Option<i64>,    // Seconds; None = no forced expiry
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub is_retraction: bool, // True if this observation retracts a previous belief
    // … temporal validity fields
}
```

The `memory_layer()` method returns the routing layer (falling back from `internal_layer` to
the layer implied by `memory_type`).

### `Provenance`

```rust
pub struct Provenance {
    pub source_type: SourceType,    // Chat | File | Tool | System | External
    pub source_id: String,
    pub source_label: Option<String>,
    pub observed_by: Option<String>,
    pub trust_weight: f32,          // 0.0–1.0
}
```

### `RetrievalQuery`

```rust
pub struct RetrievalQuery {
    pub text: String,
    pub intent: QueryIntent,        // CurrentFact | HistoricalFact | PreferenceLookup | …
    pub entity: Option<String>,
    pub slot: Option<String>,
    pub top_k: usize,
    pub min_confidence: Option<f32>,
    pub scope: Option<Scope>,
}
```

### `QueryIntent`

| Variant | Routing behaviour |
|---|---|
| `CurrentFact` | Prioritises Belief layer, recency-boosted |
| `HistoricalFact` | Searches Episode layer |
| `PreferenceLookup` | Searches Belief + SelfModel layers |
| `TaskState` | Searches GoalState layer |
| `EpisodicRecall` | Searches Episode layer with temporal relevance |
| `SemanticBackground` | Broad search across all layers |

### `MemoryType` and `MemoryLayer`

`MemoryType` is the semantic category (Trace / Episode / Fact / Preference / GoalState).
`MemoryLayer` is the physical storage target (Trace / Episode / Belief / GoalState / SelfModel / Procedure).
`MemoryType::memory_layer()` provides the default mapping.

---

## Ingest pipeline

```
ingest(CandidateMemory)
  │
  ├─ MemoryClassifier::classify()
  │    assigns memory_type, memory_layer, confidence normalisation
  │    emits AuditEvent("classification")
  │
  ├─ MemoryPromoter::promote()
  │    promotion gate: Reject | StoreTrace | Promote
  │    Promote → creates DurableMemory record, assigns memory_id
  │    emits AuditEvent("promotion")
  │
  ├─ if Belief && is_retraction → BeliefUpdater::handle_retraction()
  ├─ if Belief && not retraction → BeliefUpdater::update_or_create()
  │    may mark conflicting beliefs as Disputed or Stale
  │    emits AuditEvent("belief_update")
  │
  ├─ route to layer store (episode_store, belief_store, …)
  │
  └─ return memory_id (or None if Reject)
```

### `PromotionDecision`

| Decision | Meaning |
|---|---|
| `Reject` | Observation dropped; below confidence/salience threshold |
| `StoreTrace` | Stored in Trace layer only; not promoted to durable record |
| `Promote` | Stored in the appropriate durable layer with a stable ID |

---

## Belief management

Beliefs are the primary long-lived memory type. Each belief carries:

- `entity` + `slot` — the subject-property pair it asserts
- `value` — the current asserted value
- `status` — `Active | Disputed | Stale | Retracted`
- `confidence` — float 0–1
- `valid_from` / `valid_to` — optional temporal validity window

When a new observation contradicts an existing Active belief for the same `(entity, slot)`,
`BeliefUpdater` marks the older belief as `Disputed` and the audit log records both events.
A retraction (`is_retraction = true`) transitions the belief to `Retracted`.

---

## Procedure lifecycle

`ProcedureStore` tracks learned multi-step processes. Each procedure has a status
(`Draft → Active → Deprecated → Archived`) and every transition requires an explicit
`ProcedureStatusTransition` with a human-readable reason. The full transition history is
stored alongside the procedure so the `MemoryRetriever` can surface it as context.

---

## Retention and consolidation

`MemoryDecay` applies TTL-based and salience-weighted decay to Trace and Episode entries.
`ConsolidationEngine` periodically promotes high-frequency episode patterns to Beliefs.
`MemoryCompactor` removes fully expired entries and defragments the underlying store.

Retention policies are configured per-layer via `policy` module types. The defaults are:

- Trace: 72 h TTL, no consolidation upward
- Episode: 30 d TTL, eligible for Belief consolidation after ≥ 3 corroborating observations
- Belief/GoalState/SelfModel/Procedure: no automatic expiry (manual retraction or archival)

---

## Audit log

Every ingest action emits an `AuditEvent` to `AuditLogger`:

```rust
pub struct AuditEvent {
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub action: String,           // "classification" | "promotion" | "belief_update" | …
    pub candidate_id: Option<String>,
    pub memory_id: Option<String>,
    pub belief_id: Option<String>,
    pub query_text: Option<String>,
    pub details: BTreeMap<String, String>,
}
```

The audit log is append-only and stored in its own track so it can be queried independently of
the main memory search path.

---

## Public exports

From `src/agent_memory/mod.rs`:

```rust
pub use memory_controller::MemoryController;
// All other types are accessed via their submodules:
// agent_memory::schemas::{CandidateMemory, DurableMemory, RetrievalHit, RetrievalQuery, …}
// agent_memory::enums::{MemoryLayer, MemoryType, BeliefStatus, QueryIntent, …}
// agent_memory::errors::Result
```
