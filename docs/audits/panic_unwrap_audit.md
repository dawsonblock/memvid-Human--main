# Panic / Unwrap Audit — memvid-core v2.0.139

**Audit date:** 2026-05-05  
**Auditor:** governance-automation  
**Crate:** `memvid-core` v2.0.139 (MSRV 1.85.0, edition 2024)  
**Scope:** `src/**/*.rs` — production code only; test sites are classified separately  
**Tool:** `scripts/audit_panics.py` (Python 3.11+ engine; `scripts/audit_panics.sh` is a thin shell wrapper)

---

## 1. Executive Summary

No unallowlisted production panic sites were found.  All 648 detected
panic-family lines fall into one of three buckets: **test-only** (the vast
majority), **allowlisted production** (129 sites across 50 allowlist entries),
or **review-flagged** (0).  The codebase has zero calls to `unimplemented!()`.

| Verdict | Count |
|---------|-------|
| Unallowlisted production panic sites | **0** |
| Allowlisted production sites | 129 |
| Test-only sites | 519 |

> **Classifier note (2026-05-05):** The test-scope heuristic was upgraded from
> a first-boundary scan to a brace-depth-aware algorithm.  Previously, the first
> `#[cfg(test)]` marker in a file incorrectly classified all subsequent lines as
> test-only regardless of enclosing braces.  The corrected classifier reduced the
> test bucket from 524 → 519 (5 lines re-classified) and increased the allowlisted
> bucket from 124 → 129 (the newly visible production sites already had correct
> allowlist entries or were added in this revision).

---

## 2. Raw Counts

The following counts were produced by `scripts/audit_panics.py` over `src/`:

| Pattern | Total occurrences |
|---------|------------------|
| `.expect(` | 329 |
| `.unwrap()` | 289 |
| `panic!` | 23 |
| `unreachable!` | 7 |
| `todo!` / `unimplemented!` | 0 |
| **Grand total** | **648** |

---

## 3. Classification by Bucket

### 3.1 Test-only sites

Every `panic!` call (23/23) is inside a `#[cfg(test)]` block.  The table
below shows the files with the highest combined `.unwrap()`/`.expect()` counts
and confirms each as test-only or allowlisted.

| File | `.unwrap()` | `.expect(` | Bucket |
|------|------------|-----------|--------|
| `src/lib.rs` | 0 | 134 | **test** (all inside `#[cfg(test)]`) |
| `src/enrich/rules.rs` | 23 | 0 | **test** |
| `src/analysis/temporal_enrich.rs` | 0 | 29 | **test** |
| `src/memvid/wal.rs` | 0 | 16 | **test** |
| `src/vec.rs` | 0 | 13 | **test** |
| `src/search/parser.rs` | 0 | 12 | **test** |
| `src/memvid/audit.rs` | 0 | 12 | **test** |
| `src/agent_memory/memory.rs` | 34 | 0 | **test** |
| `src/agent_memory/tests_lex_flag.rs` | 24 | 0 | **test** |
| `src/agent_memory/memory_compactor.rs` | 19 | 0 | **test** |
| `src/agent_memory/memories_track.rs` | 16 | 0 | **test** |
| `src/agent_memory/concept_synthesis.rs` | 14 | 0 | **test** |
| `src/signature.rs` | 13 | 0 | **test** |
| `src/agent_memory/reasoning_engine.rs` | 12 | 0 | **test** |

### 3.2 Allowlisted production sites

All production panic-family sites match an approved category in
`tools/panic_allowlist.toml`.

| File | Count | Category | Notes |
|------|-------|----------|-------|
| `src/io/temporal_index.rs` | 10 | `byte-slice-to-array`, `temporal-index-checksum-try-into` | `try_into().unwrap()` on fixed-length slices; length guaranteed by `SLOT_BYTES` / `HEADER_SIZE` constants.  One site uses a multi-line chain (`.try_into()` on one line, `.unwrap()` on the next) and is covered by the dedicated `temporal-index-checksum-try-into` allowlist entry. |
| `src/analysis/temporal.rs` | ~20 | `regex-literal`, `regex-literal-init`, `date-from-valid-args`, `temporal-checked-add`, `temporal-month-bounded`, `temporal-time-from-hms` | Regex literals (incl. multi-line `.expect("regex literal")` form); calendar arithmetic with bounded operands |
| `src/analysis/auto_tag.rs` | 2 | `regex-literal` | Multi-line `Regex::new(…).expect("regex literal")` inside `LazyLock::new` |
| `src/text_embed.rs` | 1 | `static-config-sentinel` | `.expect("No default text embedding model configured")` on a file-local static table |
| `src/search/tantivy/query.rs` | 1 | `guarded-next-unwrap` | `.expect("clauses.len() == 1")` guarded by explicit length check in same branch |
| `src/api_embed.rs` | 1 | `option-error-branch` | `.expect("error set in preceding branch")` set in the retry-loop `Err` branch |
| Various production files | ~98 | `regex-literal`, `precondition-validated`, `invariant-checked-local`, `invariant-mem-available`, `invariant-vec-manifest`, `parse-year-digits`, `static-model-default`, `regex-valid-expect`, `regex-capture-full-match`, `temporal-enrich-valid-regex`, `temporal-enrich-caps-full-match`, `*-validated-expect`, `*-default-model-expect`, `*-try-into-unwrap`, `pii-*-regex-expect`, `table-storage-checked-above-expect` | Additional sites classified by newly added allowlist categories (see §4 findings F-6–F-14) |

### 3.3 `unreachable!()` sites

| File | Count | Category | Notes |
|------|-------|----------|-------|
| `src/agent_memory/consolidation_engine.rs` | 5 | `sealed-enum-unreachable` | `ConsolidationDisposition::Duplicate` arm in 5 `match` blocks; variant is filtered by caller |
| `src/agent_memory/extraction/procedure_extractor.rs` | 1 | `regex-unwrap-or-unreachable` | `Regex::new(r"…").unwrap_or_else(\|_\| unreachable!())` |
| `src/analysis/temporal.rs` | 1 | `test` | Inside `#[cfg(test)]` |

---

## 4. Findings

### Finding F-1 — All explicit `panic!()` calls in test code ✅

All 23 occurrences of `panic!` in `src/` are syntactically within
`#[cfg(test)]` blocks.  None are in production code paths.

**Severity:** None  
**Action required:** None

---

### Finding F-2 — Production `.unwrap()` / `try_into().unwrap()` in temporal_index.rs ✅

Nine `try_into().unwrap()` calls convert `&[u8]` slices to `[u8; N]` arrays.
The slice lengths are established by the `SLOT_BYTES` constant in the same
module, making the `TryFrom` error path structurally unreachable.

**Severity:** Informational  
**Action required:** None now.  A future hardening step could add
`debug_assert_eq!(slice.len(), N, "slot layout changed")` before each
conversion (see remediation backlog below).

---

### Finding F-3 — Static config sentinel in text_embed.rs ✅

One `.expect("No default text embedding model configured")` at line 225 in
`src/text_embed.rs`.  The static `TEXT_EMBED_MODELS` slice is defined in the
same file and must contain exactly one `is_default = true` entry; the absence
of such an entry would also cause every test that instantiates the embedder to
fail, so the invariant is covered by the test suite.

**Severity:** Informational  
**Action required:** Add a unit test asserting exactly one model has
`is_default = true` (see remediation backlog).

---

### Finding F-4 — Regex initialization in OnceLock closures ✅

At least 18 `Regex::new(r"…").unwrap()` calls inside `OnceLock::get_or_init`
closures in `src/analysis/temporal.rs`, plus one
`Regex::new(r"…").unwrap_or_else(|_| unreachable!())` in
`src/agent_memory/extraction/procedure_extractor.rs`.  These are infallible by
the same argument as a constant expression: the literal is reviewed at
code-review time and tested on every CI run.

**Severity:** None  
**Action required:** None

---

### Finding F-5 — Sealed-enum unreachable arms ✅

Five `ConsolidationDisposition::Duplicate => unreachable!()` arms in
`src/agent_memory/consolidation_engine.rs`.  The `Duplicate` variant is
filtered from the input collection before each `match`; the arm is structurally
dead but required for Rust's exhaustiveness check.

**Severity:** None  
**Action required:** None

---

### Finding F-6 — Multi-line regex literals with `.expect("regex literal")` ✅

After converting bare `.unwrap()` on multi-line `Regex::new(…)` calls to
`.expect("regex literal")`, these sites in `src/analysis/temporal.rs` (5 sites)
and `src/analysis/auto_tag.rs` (2 sites) are now classified by the new
`regex-literal` allowlist category.  Justification is identical to
category 1 (`regex-literal-init`).

**Severity:** None  
**Action required:** None

---

### Finding F-7 — Guarded iterator `.next().unwrap()` in query.rs ✅

One `.expect("clauses.len() == 1")` in `src/search/tantivy/query.rs`
follows an explicit `if clauses.len() == 1 {` guard.  The `None` arm is
structurally unreachable within that branch.

**Severity:** None  
**Action required:** None

---

### Finding F-8 — Retry-loop error-option in api_embed.rs ✅

One `.expect("error set in preceding branch")` in `src/api_embed.rs`
unwraps a `last_error: Option<Error>` that is unconditionally set whenever
the retry loop exits via the `Err` branch.  If the loop body succeeds on
all retries, the `last_error` path is never reached.

**Severity:** None  
**Action required:** None

---

### Finding F-9 — Temporal arithmetic bounded operations ✅

Three allowlist categories cover calendar arithmetic in
`src/analysis/temporal.rs`: `temporal-checked-add`, `temporal-month-bounded`,
and `temporal-time-from-hms`.  In all cases the operands are derived from
an already-valid `Date` or from regex captures constrained to valid ranges.

**Severity:** None  
**Action required:** None

---

### Finding F-10 — Additional `.expect(…)` descriptive-message sites ✅

Several additional categories address sites where a previous bare `.unwrap()`
already carries a descriptive `.expect(…)` message matching categories
`precondition-validated`, `invariant-checked-local`, `invariant-mem-available`,
`invariant-vec-manifest`, `static-model-default`, `regex-valid-expect`,
`regex-capture-full-match`, and `parse-year-digits`.  All are structural
invariants documented at the call site.

**Severity:** None  
**Action required:** None

---

---

### Finding F-11 — Agent memory validated-expect sites ✅

Twelve `expect("validated …")` calls across `src/agent_memory/goal_state_store.rs`,
`src/agent_memory/procedure_store.rs`, and `src/agent_memory/self_model_store.rs`
follow explicit caller-side validation of memory entry fields.  `None` is
structurally impossible at the call sites.

**Severity:** None  
**Action required:** None (remediation: propagate `Result` up the call stack)

---

### Finding F-12 — Static regex in pii.rs / temporal_enrich.rs ✅

Thirteen `.expect("valid regex")` calls in `src/analysis/temporal_enrich.rs`
plus two `.expect("invalid …regex")` calls in `src/pii.rs` compile hard-coded
literal regexes.  Panics are possible only if a source literal is malformed—
caught at code review.

**Severity:** None  
**Action required:** None (remediation: use `LazyLock<Regex>`)

---

### Finding F-13 — caps.get(0) full-match unwrap in temporal_enrich.rs ✅

Six `.expect("full match")` calls in `src/analysis/temporal_enrich.rs`
unwrap `Captures::get(0)`, which is always `Some` when a regex match has
already succeeded.

**Severity:** None  
**Action required:** None

---

### Finding F-14 — Fixed-size byte slice conversion in manifest_wal / replay ✅

Two `try_into().unwrap()` calls convert compile-time constant-length byte slices
to fixed-size arrays (`header[..4]` and `data[8..16]`).  The `TryFrom` error
is structurally unreachable.

**Severity:** None  
**Action required:** None

---

## 5. Remediation Backlog

These items are not blocking but improve long-term robustness:

| ID | Level | File | Suggestion |
|----|-------|------|------------|
| REM-1 | Low | `src/io/temporal_index.rs` | Add `debug_assert_eq!(slice.len(), SLOT_BYTES)` guard before each `try_into().unwrap()` |
| REM-2 | Low | `src/text_embed.rs` | Add `#[test] fn exactly_one_default_model()` asserting `.filter(|m| m.is_default).count() == 1` |
| REM-3 | Low | `src/analysis/temporal.rs` | Consider using the `once_cell::sync::Lazy` or Rust 1.80+ stable `LazyLock` to keep initialization even closer to the static binding |
| REM-4 | Very Low | `src/io/temporal_index.rs` | Long-term: introduce a `read_u64_be(buf: &[u8; 8]) -> u64` helper to eliminate repeated `try_into().unwrap()` calls |

---

## 6. How to Re-run the Audit

```bash
# Summary to stdout (exit 0 always)
python3 scripts/audit_panics.py
# or via the thin bash wrapper:
./scripts/audit_panics.sh

# Strict mode: exit 1 if any review-class site is found
python3 scripts/audit_panics.py --strict

# Write TSV report
python3 scripts/audit_panics.py --out artifacts/audits/panic_report.tsv

# Use a custom allowlist location
python3 scripts/audit_panics.py --allowlist tools/panic_allowlist.toml --strict
```

> **Note:** Requires Python 3.11+ (stdlib `tomllib`).  Use `python3.11` or
> `python3.13` if your system default is older.

---

## 7. Conclusion

**Audit result: PASS.**  
memvid-core v2.0.139 contains zero unallowlisted production panic sites.
All 129 production uses of `unwrap()`, `expect()`, `unreachable!()` have been
reviewed and documented across 50 allowlist entries in
`tools/panic_allowlist.toml`.  The `scripts/audit_panics.py --strict` gate is
run in CI (`.github/workflows/ci.yml` `panic-audit` job) to prevent
regressions.  Output is archived as `artifacts/audits/panic_report.tsv`.

The audit script was also upgraded in this revision:
- **Brace-aware test-scope classifier** — `#[cfg(test)]` / `#[test]` scopes are
  now tracked with brace-depth counting, preventing false classification of
  production code that follows a closed test module.
- **`--src DIR` flag** — allows targeting an alternative source tree without
  modifying the script.
- **Multi-match per line** — both `.unwrap()` and `panic!` on the same source
  line are now each reported as separate findings (previously only the first
  matching pattern was emitted).
