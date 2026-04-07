# Agent Memory

`src/agent_memory` is a bounded, policy-governed layer built on top of `Memvid`. It does not add
new storage backends or widen the architecture. It adds deterministic routing, promotion,
consolidation, retrieval, retention, and audit behavior around six internal layers.

## Layer Model

Public memory types map to internal storage layers as follows:

- `Trace -> Trace`
- `Episode -> Episode`
- `Fact -> Belief`
- `Preference -> SelfModel`
- `GoalState -> GoalState`

`Procedure` is an internal layer reached only through explicit routing and consolidation.

| Layer | Role | Logical identity and read behavior | Default retention |
| --- | --- | --- | --- |
| `Trace` | Raw observations, audit traces, lifecycle traces | Archival only; never promoted directly to higher-order durable layers | TTL 3 days, decay 0.18/day |
| `Episode` | Event evidence and historical records | Event evidence layer; also used as fallback when a durable promotion is denied but the observation is worth keeping | TTL 30 days, decay 0.04/day |
| `Belief` | Current factual state plus supporting evidence memories | Active truth is maintained per `(entity, slot)` while supporting evidence remains retrievable | No default TTL, decay 0.005/day |
| `GoalState` | Active tasks, blockers, and waiting states | Latest logical record per `(entity, slot)`; active reads keep only current active or blocked states | TTL 14 days, decay 0.03/day |
| `SelfModel` | Durable preferences, constraints, and operating traits | One logical trait per `(entity, slot)`; repetition reinforces instead of duplicating | No default TTL, decay 0.002/day |
| `Procedure` | Reusable workflows and operational habits | One logical procedure per `workflow_key`; lifecycle can be `active`, `cooling_down`, or `retired` | TTL 90 days, decay 0.01/day |

## Policy-Backed Thresholds

`PolicySet` is the single semantic source for promotion, repetition, and stabilization knobs.
The default score is:

```text
score = (confidence * 0.6) + (salience * 0.4)
```

Current default thresholds from `policy.rs` are:

- Reject below `0.25`
- Trace-only below `0.35`
- Episode promotion threshold `0.65`
- Belief promotion threshold `0.75`
- Self-model promotion threshold `0.70`
- Goal-state promotion threshold `0.65`
- Procedure promotion threshold `0.72`
- Minimum self-model repetitions `2`
- Minimum successful procedure repetitions `2`
- Minimum recurring blocker repetitions `3`
- Minimum belief stabilization repetitions `2`
- Consolidation window `30` days
- Minimum belief stability span `3` days
- Trusted belief source weight `0.8`
- Trusted self-model source weight `0.9`

`PromotionContext` is built before promotion and carries:

- `belief_evidence_count`
- `self_model_evidence_count`
- `goal_state_evidence_count`
- `procedure_success_count`
- `procedure_failure_count`
- `verified_source`
- `seeded_by_system`

## Ingest And Destination-Aware Promotion

`MemoryController` is the single governed authority for writes and reads. The ingest path is:

```text
ingest(candidate)
  -> classify
  -> build PromotionContext from current store state
  -> promote with destination-aware rules
  -> write supporting episode when applicable
  -> write durable layer if allowed
  -> update beliefs when applicable
  -> run consolidation
  -> emit audit events for every step
```

Promotion is destination-aware rather than score-only.

- `Trace`: archival only. Trace candidates can be retained as trace but are never promoted directly into durable higher-order layers.
- `Episode`: singleton promotion is allowed only for event-like observations.
- `GoalState`: singleton promotion is allowed for structured task-state or blocker observations with explicit goal-state semantics.
- `Belief`: requires `entity`, `slot`, and `value`, then one of: verified source, repeated evidence at or above threshold, or trusted source at or above the belief trust floor.
- `SelfModel`: requires `entity`, `slot`, and `value`, then either repeated stable evidence or an explicit durable preference or constraint statement from a verified source or a trusted `System` or `Tool` source.
- `Procedure`: requires a `workflow_key` and either repeated successful workflow evidence or explicit system seeding.

When durable promotion is denied but the observation still has useful structure or event semantics,
the controller stores it as `Episode` evidence instead of treating it as current truth. This is the
normal fallback for denied belief, self-model, and procedure promotion. Only low-scoring or
unstructured leftovers fall all the way back to `Trace`.

When a non-episode observation does promote durably, the controller records a supporting episode
first and attaches that episode id to the durable memory. This keeps history and truth separated
while preserving evidence linkage.

## Durable Layer Behavior

### Belief

Belief state is split between supporting evidence memories and an active `BeliefRecord` for the
current `(entity, slot)` value. Contradictions and retractions flow through `BeliefUpdater`, which
can mark beliefs as `active`, `disputed`, `stale`, or `retracted` while leaving evidence available
for historical retrieval and consolidation.

### GoalState

`GoalStateStore` gives goal-state memories logical identity by `(entity, slot)`. New writes for the
same key reuse the prior memory id and replace the current logical view. Active queries surface only
the latest record per key whose status is one of:

- `active`
- `blocked`
- `waiting_on_user`
- `waiting_on_system`

`completed`, `inactive`, and `stale` goal states stay in storage but are filtered out of active
task-state recall.

### SelfModel

`SelfModelStore` gives self-model memories logical identity by `(entity, slot)`.

- Repeated same-value evidence reinforces one logical entry.
- Reinforcement increments `reinforcement_count` and records `last_reinforced_at`.
- Supporting evidence ids are merged instead of duplicating the logical trait.
- Stronger contradictions can update the current trait on the same logical id.
- Weaker contradictions are stored as `disputed` and do not replace the stable active trait.

This keeps self-model memory narrow and durable instead of becoming a scrapbook of user remarks.

### Procedure

`ProcedureStore` uses `workflow_key` as the logical identity.

- Relearning the same workflow reuses the same procedure id.
- Success increments `success_count` and adjusts confidence.
- Failures increment `failure_count` immediately.
- Effective lifecycle is derived from observed outcomes and can be `active`, `cooling_down`, or `retired`.
- Retired procedures are filtered from task-state retrieval; cooling-down procedures are still retrievable but ranked lower.

## Consolidation

`ConsolidationEngine` promotes repeated bounded patterns into searchable consolidation traces. It
currently consolidates:

- stable self-model evidence
- stable belief windows
- recurring blockers
- repeated workflow success
- workflow failure that degrades an existing procedure lifecycle

Every consolidation branch follows the same model:

- threshold semantics using policy-backed `>=` checks where repetition matters
- a semantic key that identifies the meaning of the pattern
- an evidence fingerprint that identifies the exact source evidence and threshold/window inputs
- a disposition of `initial`, `reinforcement`, or `duplicate`

Disposition behavior is:

- `initial`: first time this semantic pattern has been recorded
- `reinforcement`: same semantic pattern, but supported by new evidence
- `duplicate`: same semantic pattern and same evidence fingerprint; no new record is emitted

This makes consolidation idempotent on rerun while distinguishing first-time promotion from later
reinforcement.

Procedure-related consolidation records also preserve workflow metadata, outcome, and any status
transition caused by the new evidence.

## Retrieval And Ranking

`MemoryRetriever` is answer-first. Each intent returns a primary direct layer hit first, then a
small bounded amount of support evidence, then archive fallback only when needed.

Current intent behavior is:

- `CurrentFact`: active belief first, then up to `3` support memories and up to `3` supporting episodes, then archive fallback.
- `HistoricalFact`: recent episodes first, then bounded prior-state support, then archive fallback.
- `PreferenceLookup`: self-model first, then up to `3` bounded support hits and linked support episodes, then archive fallback.
- `TaskState`: active goals first, then up to `3` aligned episodes, then up to `2` aligned procedures, then archive fallback.
- procedural help under `SemanticBackground`: up to `3` direct procedures first, then up to `3` successful supporting episodes.
- procedure lifecycle history under `SemanticBackground` or `EpisodicRecall`: direct answers come from stored lifecycle trace entries.

Support hits are marked with `retrieval_role = support_evidence`. Direct hits are marked with
`retrieval_role = direct_answer`. Archive fallback hits are marked with `retrieval_role = archive_fallback`.

`Ranker` enforces layer-role hierarchy rather than generic additive math.

- Direct answers outrank support evidence.
- Support evidence outranks archive fallback.
- Current fact strongly prefers belief.
- Preference lookup strongly prefers self-model.
- Task state strongly prefers goal-state, with blocked and waiting states boosted over generic active goals.
- Procedural help prefers procedure memory.
- Historical queries prefer episodes over current belief.
- Recency matters strongly for episodes and goal-state, and much less for stable beliefs, self-model, and procedures.
- Cooling-down procedures are penalized; retired procedures are heavily penalized or filtered depending on the query path.

Before returning results, retrieval performs semantic dedup rather than id-only dedup. The dedup key
combines entity, slot, value, `workflow_key`, source identity, and a coarse time bucket where
possible, then falls back to belief id, memory id, or normalized text.

## Adapter And Audit Preservation

`MemvidStore` preserves the metadata needed by ranking, dedup, support linkage, and lifecycle logic.
Durable memories are written with agent-prefixed metadata for:

- memory id and candidate id
- entity, slot, and value
- memory layer and memory type
- source id, source type, and source weight
- confidence and salience
- stored, event, and validity timestamps
- arbitrary workflow, support, and layer-specific metadata

On read, those fields are reconstructed into `DurableMemory` and `RetrievalHit`. Query time is not
injected as if it were memory time; retrieval uses persisted stored and event timestamps.

The audit trail is append-only and explicit. `MemoryController` emits events such as:

- `classification`
- `promotion`
- `trace_stored`
- `episode_stored`
- `memory_stored`
- `belief_updated`
- `procedure_learned`
- `procedure_status_changed`
- `retrieval`

Promotion audit details explain which layer was chosen, the score and threshold, route basis such as
`verified_source`, `trusted_source`, `repeated_evidence`, or `system_seeded`, and any fallback layer
when durable promotion is denied.

Procedure lifecycle changes are also persisted as searchable trace entries with explicit
`transition_reason` values such as `success`, `failure`, or `reconciliation`, so lifecycle history
is visible through both audit and retrieval paths.
