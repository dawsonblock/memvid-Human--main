# Benchmarks

This document describes the benchmark suites shipped with `memvid-core` and
explains how to run them and interpret the output.

---

## Suites

### 1. Extraction Pipeline (`extraction_pipeline_benchmark`)

Located at `benches/extraction_pipeline_benchmark.rs`.  
Measures the per-call latency of each stage in the `agent_memory` extraction
subsystem using [Criterion](https://github.com/bheisler/criterion.rs).

| Benchmark | What it measures |
|---|---|
| `preference_extractor` | `PreferenceExtractor::extract` on an 80-character string with three preference signals (prefer / like / hate) |
| `claim_extractor` | `ClaimExtractor::extract` on a 120-character string exercising is-are, has, colon, instruction, and negation dispatch paths |
| `pipeline_short` | Full `RawInputProcessor::process` round-trip on a single-sentence input ("I prefer dark mode.") |
| `pipeline_medium` | Full `RawInputProcessor::process` round-trip on a seven-sentence paragraph covering all major extractor paths |
| `deduplication` | `MergedExtractionValidator::deduplicate` over 50 candidates (25 duplicates of one key + 25 unique keys) |

**Run all extraction benchmarks:**

```bash
cargo bench --bench extraction_pipeline_benchmark
```

**Run a single group by filter:**

```bash
cargo bench --bench extraction_pipeline_benchmark -- preference_extractor
cargo bench --bench extraction_pipeline_benchmark -- claim_extractor
cargo bench --bench extraction_pipeline_benchmark -- pipeline_short
cargo bench --bench extraction_pipeline_benchmark -- pipeline_medium
cargo bench --bench extraction_pipeline_benchmark -- deduplication
```

Criterion writes HTML reports to `target/criterion/` after each run.

---

### 2. Search Precision (`search_precision_benchmark`)

Located at `benches/search_precision_benchmark.rs`.  
Measures precision and recall metrics for the full-text (`lex`) search path.

```bash
cargo bench --bench search_precision_benchmark
```

---

### 3. Vec Search / HNSW (`vec_search_benchmark`)

Located at `benches/vec_search_benchmark.rs`.  
Requires the `hnsw_bench` feature flag (not part of the default feature set).

```bash
cargo bench --bench vec_search_benchmark --features hnsw_bench
```

---

## Running all benchmarks

```bash
# All benchmarks that compile with the standard feature set
cargo bench --features "lex,pdf_extract,simd"

# Including the HNSW bench
cargo bench --features "lex,pdf_extract,simd,hnsw_bench"
```

> **Tip:** Add `-- --save-baseline my_branch` to save a named baseline and
> `-- --load-baseline main` to compare against it.

---

## Compile-only check (no execution)

Use `--no-run` to verify that bench code compiles without spending time
running iterations — useful in CI:

```bash
cargo bench --bench extraction_pipeline_benchmark --no-run
```

---

## MSRV note

The benchmarks are compiled with the same toolchain as the library (MSRV
**1.85.0** per `rust-toolchain.toml`).  Criterion 0.8 requires Rust ≥ 1.65,
so there is no MSRV conflict.
