# Agent Memory

`src/agent_memory` is a bounded, policy-governed layer built on top of `Memvid`. It does not add
new storage backends or widen the architecture. It adds deterministic routing, promotion,
consolidation, retrieval, retention, and audit behavior around six internal layers.

This subsystem is always compiled and is part of the supported public surface of `memvid-core`.
The canonical entry point is `MemoryController`; lower-level stores and helpers remain public for
advanced integrations and for the integration tests that prove the policy model.

Throughout this repository, "human memory" means a bounded operational memory analogue for agents:
evidence, current facts, task state, self-model, and procedures. It does not imply a claim of
human cognition fidelity.

## Public Boundary

For most callers, the intended public path is:

- build a `CandidateMemory`
- ingest through `MemoryController`
- retrieve through `RetrievalQuery`
- tune compatibility thresholds through `PolicySet`
- inspect or supply protected governance through `PolicyProfile`

Dedicated stores remain public because the crate tests and some advanced integrations need direct
logical-state access, but controller-governed ingest is the canonical write path.

Typed retrieval through `RetrievalQuery` remains the canonical read path. For obvious text-only
queries, `MemoryController::retrieve_text(...)` and `RetrievalQuery::from_text(...)` provide a
small rule-based convenience wrapper around `QueryIntentDetector`; they are not the authoritative
semantic router and do not replace the typed contract.

`PolicyProfile` is protected governance state, not ordinary memory. It is layered alongside the
compatibility-first `PolicySet` and is intended to hold hard constraints, soft weights, and
machine-readable rejection reason codes outside normal episode ingest or retrieval paths.

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

`PolicySet` remains the compatibility-first surface for promotion, repetition, and stabilization
knobs. `PolicyProfile` wraps those defaults into a versioned governance object with:

- `HardConstraints` for fail-closed rules such as structured identity requirements, trusted
  singleton promotion restrictions, protected self-model rules, and procedure creation guards
- `SoftWeights` for explicit scoring inputs that later phases can use for explainable retrieval
- `ReasonCode` values for machine-readable rejection audits

Current ingest still preserves the existing `PolicySet` defaults, but the controller now records
machine-readable rejection causes in addition to human-readable reasons.

`PolicySet` remains the semantic source for promotion, repetition, and stabilization knobs.
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
- `GoalState`: requires non-empty `entity`, `slot`, and `value` after trimming, then singleton promotion is allowed for structured task-state or blocker observations with explicit goal-state semantics.
- `Belief`: requires non-empty `entity`, `slot`, and `value` after trimming, then one of: verified source, repeated evidence at or above threshold, or an explicitly trusted singleton source class. The current policy restricts trusted singleton belief promotion to `System` and `Tool` sources that meet the belief trust floor; high-trust `Chat` evidence still requires repetition unless it is verified.
- `SelfModel`: requires non-empty `entity`, `slot`, and `value` after trimming, then either repeated stable evidence or an explicit durable preference or constraint statement from a verified source or a trusted `System` or `Tool` source. Singleton self-model promotion is limited to recognized durable slot categories rather than arbitrary preference-like text.
- `Procedure`: requires non-empty `entity`, `slot`, `value`, and a `workflow_key` after trimming, plus either repeated successful workflow evidence or explicit system seeding.

When durable promotion is denied but the observation still has useful structure or event semantics,
the controller stores it as `Episode` evidence instead of treating it as current truth. This is the
normal fallback for denied belief, self-model, and procedure promotion. Only low-scoring or
unstructured leftovers fall all the way back to `Trace`.

Every blocked durable promotion now emits both the existing `promotion` audit event and a dedicated
`policy_rejected` audit event with a typed `reason_code`. The human-readable `reason` field is
preserved for compatibility; the machine-readable code is the stable governance signal for tests
and downstream tooling.

Whitespace-only structured fields are treated as absent rather than as valid structure. Dangerous
durable layers fail closed on those inputs instead of silently persisting empty-string entity, slot,
value, or workflow identity fields.

When a non-episode observation does promote durably, the controller records a supporting episode
first and attaches that episode id to the durable memory. This keeps history and truth separated
while preserving evidence linkage.

## Durable Layer Behavior

### Belief

Belief state is split between supporting evidence memories and an active `BeliefRecord` for the
current `(entity, slot)` value. Contradictions and retractions flow through `BeliefUpdater`, which
can mark beliefs as `active`, `disputed`, `stale`, or `retracted` while leaving evidence available
for historical retrieval and consolidation.

Phase 3 keeps the existing enum semantics but makes current interpretation explicit:

- `opposing_memory_ids` remains the contradiction lineage for the current interpretation
- `contradictions_observed` counts distinct contradiction events against the current belief
- `last_contradiction_at` records when the current interpretation most recently became contested
- `time_to_last_resolution_seconds` records how long the most recent contest lasted before
  reinforcement, replacement, or retraction resolved it

Belief revision now distinguishes three controller-owned outcomes without deleting history:

- same-value reinforcement keeps the current belief id and can restore a `disputed` belief to
  `active` when corroborating evidence arrives
- higher-trust comparable evidence replaces the current interpretation, marks the old belief
  `stale`, and starts a new active belief id
- weaker contradictory evidence marks the current belief `disputed` instead of silently replacing it

Current-fact retrieval prefers clean active beliefs. If the latest interpretation is contested and
there is no active replacement, retrieval can still surface that belief as a direct answer with
`belief_retrieval_status=contested`, contradiction counters, timing metadata, and bounded opposing
evidence so callers can inspect uncertainty explicitly.

### GoalState

`GoalStateStore` gives goal-state memories logical identity by `(entity, slot)`. New writes for the
same key reuse the prior memory id and replace the current logical view. Active queries surface only
the latest record per key whose status is one of:

- `active`
- `blocked`
- `waiting_on_user`
- `waiting_on_system`

`completed`, `inactive`, and `stale` goal states stay in storage but are filtered out of active
task-state recall. Malformed rows with blank structured fields are ignored during typed projection,
and newer malformed rows do not hide older valid active state.

### SelfModel

`SelfModelStore` gives self-model memories logical identity by `(entity, slot)`.

Self-model records now carry two additive governance fields:

- `stability_class`: `stable_directive` or `flexible_preference`
- `update_requirement`: `trusted_or_corroborated` or `reinforcement_allowed`

The current default mapping is intentionally narrow:

- `Constraint`, `Value`, and `CapabilityLimit` are treated as `stable_directive`
- `Preference`, `ResponseStyle`, `RiskTolerance`, `ToolPreference`, `ProjectNorm`, and `WorkPattern`
  are treated as `flexible_preference`

- Repeated same-value evidence reinforces one logical entry.
- Reinforcement increments `reinforcement_count` and records `last_reinforced_at`.
- Supporting evidence ids are merged instead of duplicating the logical trait.
- Stronger contradictions can update the current trait on the same logical id.
- Weaker contradictions are stored as `disputed` and do not replace the stable active trait.

Stable directives are controller-protected before persistence:

- a stable directive cannot be created or overwritten from a weak one-off self-model write
- stable directive updates require either a trusted source path or corroborated evidence
- weak attempts are downgraded to episode evidence on ingest and rejected on direct durable
  controller writes

Flexible preferences keep the earlier behavior: they can still reinforce or adapt under repeated or
trusted evidence while preserving provenance and contradiction history.

This keeps self-model memory narrow and durable instead of becoming a scrapbook of user remarks.
Malformed rows with blank structured fields are rejected by the dedicated store and ignored by
typed reads if they arrive through older data or bypass paths.

### Procedure

`ProcedureStore` uses `workflow_key` as the logical identity.

- Relearning the same workflow reuses the same procedure id.
- Success increments `success_count` and adjusts confidence.
- Failures increment `failure_count` immediately.
- Effective lifecycle is derived from observed outcomes and can be `active`, `cooling_down`, or `retired`.
- Retired procedures are filtered from task-state retrieval; cooling-down procedures are still retrievable but ranked lower.
- Malformed rows with blank workflow identity or other required structure fail closed and are skipped before latest-effective dedup.

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
  If no active belief exists but the latest belief is `disputed`, the contested belief can surface
  as the direct answer with explicit contradiction metadata and a ranking penalty.
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

Phase 4 makes that ranking inspectable instead of opaque. Every returned hit now carries:

- `score_components`: a stable ordered breakdown of named score terms
- `ranking_explanation`: a short explanation of why that hit ranked where it did
- per-component metadata such as `score_component_layer_match`, `score_component_content_match`,
  `score_component_goal_relevance`, `score_component_self_relevance`, `score_component_salience`,
  `score_component_evidence_strength`, `score_component_contradiction_penalty`,
  `score_component_recency`, and lifecycle penalties when relevant

The weighted terms are driven by `PolicyProfile::soft_weights()`, while a small number of
structural bonuses remain explicit to preserve answer-first routing:

- layer match for intent-aware routing
- retrieval role priority for direct answers versus support and archive fallback
- belief-priority bonus for current-fact answers sourced from the belief layer
- explicit lifecycle and expiry penalties

Phase 5 keeps that explanation contract intact while making salience maintenance dynamic.
The controller now records bounded access metadata on returned durable memories using
`retrieval_count` and `last_accessed_at`, and retention folds those signals back into the
effective salience used by ranking. Read touches do not rewrite immutable ingest time: durable
memories now keep `stored_at` as ingest chronology, `updated_at` as mutable version/activity time,
and event/history time separately. The visible score surface stays the same: the `salience`
component now reflects both write-time salience and bounded repeated-recall boosts.
That read-side write behavior is explicit policy rather than an implicit cost:
`PolicySet::persist_retrieval_touches()` defaults to `true`, so retrieval persists durable
access-touch records unless a caller disables it. When disabled, typed retrieval behavior and
returned hits stay the same, but retrieval does not append durable access-touch records.

Phase 6 extends that same controller-owned retrieval and feedback path with explicit outcome feedback.
External callers can now attach positive or negative feedback to a durable memory id or
procedure workflow key, procedure feedback reuses the existing lifecycle and success/failure
counts instead of creating a parallel score path, and generic durable memories feed feedback
back into effective salience through bounded outcome-impact adjustments.

This tranche now also supports direct `belief_id` feedback. Belief feedback stays belief-native:
it updates the persisted `BeliefRecord`, preserves the Phase 4 score surface by flowing through
bounded evidence-strength adjustment rather than a new ranking factor, and remains auditable via
the same controller-owned `outcome_feedback_recorded` event.

This keeps ranking deterministic and inspectable without widening the storage model or changing the
intent routing contract.

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
- stored, updated, event, and validity timestamps
- arbitrary workflow, support, and layer-specific metadata

On read, those fields are reconstructed into `DurableMemory` and `RetrievalHit`. Query time is not
injected as if it were memory time; retrieval uses persisted ingest, update, and event timestamps
without letting read touches move historical visibility.

Access touches no longer write a fresh durable memory body. When retrieval-touch persistence is
enabled, the store keeps retrieval metadata on a dedicated access-touch path so `retrieval_count`,
`last_accessed_at`, and effective salience can advance without creating a new durable content
version or per-hit commit cycle on every read. That access-touch history remains append-only today:
maintenance does not roll it up, supersede it, or claim logical compaction.

## Maintenance Status

`MemoryController::run_maintenance()` is the explicit maintenance surface. It:

- lists the current durable memories that governed maintenance can act on
- runs `MemoryDecay`
- returns the expired ids
- reports `MemoryCompactor` status as unsupported together with the stable unsupported reason
- emits a `maintenance` audit event through the same append-only audit sink used for other major controller operations

`MemoryCompactor` is still an explicit stub; `Memvid` owns low-level storage compaction and
governed memory does not currently add a separate logical compaction pass.

There is no hidden background maintenance loop in `MemoryController`.

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
- `maintenance`

Promotion audit details explain which layer was chosen, the score and threshold, route basis such as
`verified_source`, `trusted_source`, `repeated_evidence`, or `system_seeded`, and any fallback layer
when durable promotion is denied. After the current hardening pass, `trusted_source` for belief and
self-model singleton promotion means an allowed trusted source class, not just a large trust weight;
for self-model it also requires a recognized durable slot category rather than an arbitrary slot.

Procedure lifecycle changes are also persisted as searchable trace entries with explicit
`transition_reason` values such as `success`, `failure`, or `reconciliation`, so lifecycle history
is visible through both audit and retrieval paths.
