# Panic / Unwrap Audit — memvid-core v2.0.139

**Audit date:** 2026-05-04  
**Auditor:** governance-automation  
**Crate:** `memvid-core` v2.0.139 (MSRV 1.85.0, edition 2024)  
**Scope:** `src/**/*.rs` — production code only; test sites are classified separately  
**Tool:** `scripts/audit_panics.sh` (version introduced in this commit)

---

## 1. Executive Summary

No unallowlisted production panic sites were found.  All 650 grepped
panic-family lines fall into one of three buckets: **test-only** (the vast
majority), **allowlisted production** (25 sites across 3 files), or
**test infrastructure**.  The codebase has zero calls to `unimplemented!()`.

| Verdict | Count |
|---------|-------|
| Unallowlisted production panic sites | **0** |
| Allowlisted production sites | ~25 |
| Test-only sites | ~625 |

---

## 2. Raw Counts

The following counts were produced by `grep -r` over `src/`:

| Pattern | Total occurrences |
|---------|------------------|
| `.unwrap()` | 302 |
| `.expect(` | 316 |
| `panic!` | 23 |
| `unreachable!` | 7 |
| `todo!` | 2 (doc-comment examples only) |
| `unimplemented!` | 0 |
| **Grand total** | **650** |

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
| `src/io/temporal_index.rs` | 9 | `byte-slice-to-array` | `try_into().unwrap()` on fixed-length slices; length guaranteed by `SLOT_BYTES` constant |
| `src/analysis/temporal.rs` | ~15 | `regex-literal-init`, `date-from-valid-args` | Regex literals in `OnceLock::get_or_init`; `Date::from_calendar_date` with derived components |
| `src/text_embed.rs` | 1 | `static-config-sentinel` | `.expect("No default text embedding model configured")` on a file-local static table |

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
./scripts/audit_panics.sh

# Strict mode: exit 1 if any review-class site is found
./scripts/audit_panics.sh --strict

# Write TSV report
./scripts/audit_panics.sh --out /tmp/panic_report.tsv

# Use a custom allowlist location
./scripts/audit_panics.sh --allowlist tools/panic_allowlist.toml --strict
```

---

## 7. Conclusion

**Audit result: PASS.**  
memvid-core v2.0.139 contains zero unallowlisted production panic sites.
All production uses of `unwrap()`, `expect()`, `unreachable!()` have been
reviewed and documented.  The `scripts/audit_panics.sh --strict` gate may be
added to CI to prevent regressions.
