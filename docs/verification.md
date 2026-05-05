# Verification Guide

This document describes every test command profile used to verify `memvid-core`, along with the
MSRV, Docker labeling policy, and benchmark guidance.

---

## Test Profiles

### Default profile — `lex, pdf_extract, simd`

The canonical test run used in CI:

```bash
cargo test --features "lex,pdf_extract,simd"
```

Covers: full-text search, PDF ingestion, SIMD acceleration, agent-memory controller, retrieval
policy, touch-persistence opt-in semantics, lifecycle, replay integrity, and all structured/table
tests.

### Minimal kernel — no features

Verifies the storage kernel compiles and tests pass without any optional features:

```bash
cargo test --no-default-features
```

### MSRV — 1.85.0

The minimum supported Rust version is **1.85.0** (declared as `rust-version` in `Cargo.toml`; development uses `stable` via `rust-toolchain.toml`).

To verify on MSRV explicitly:

```bash
rustup install 1.85.0
rustup run 1.85.0 cargo build --features "lex,pdf_extract,simd"
rustup run 1.85.0 cargo test --features "lex,pdf_extract,simd"
```

### Optional / not required in standard CI

The following profiles require native libraries unavailable in the standard CI environment and are
not required for correctness verification:

| Profile | Skipped because |
|---------|-----------------|
| `--all-features` | Requires `onnxruntime` (CLIP / Whisper) |
| `--features vec` | Requires native HNSW build artifacts |

---

## Automated Baseline Script

`scripts/proof_baseline.sh` runs format check, Clippy, and all three verifiable profiles (default,
minimal, MSRV build+test via `rustup run 1.85.0`) and writes timestamped logs to `artifacts/proof/`:

```bash
bash scripts/proof_baseline.sh
# optionally: bash scripts/proof_baseline.sh --out /tmp/myproof
```

Log files follow the naming convention `baseline-<YYYYMMDDTHHMMSSZ>.log`.

---

## Retrieval-Touch Semantics

As of **v2.0.139**, `persist_retrieval_touches` and `persist_access_touches` default to **false**.

Retrieval is now read-only by default. To opt in to access-touch tracking:

```rust
// 1. Use a store with touch persistence enabled
let memvid = Memvid::open("file.mv2")?;
let store = MemvidStore::with_access_touch_persistence(memvid, true);

// 2. Build a policy that writes touch frames on retrieval
let policy = PolicySet::default().with_persist_retrieval_touches(true);

// 3. Construct the controller with the full 7-argument signature
let clock = Arc::new(SystemClock);
let controller = MemoryController::new(
    store,
    clock.clone(),
    AuditLogger::new(clock.clone()),
    MemoryClassifier,
    MemoryPromoter::new(policy.clone()),
    BeliefUpdater,
    MemoryRetriever::new(Ranker, RetentionManager::new(policy)),
);
```

Historical queries (`RetrievalQuery { as_of: Some(t), .. }`) never write touch metadata even when
the policy is fully opted in.

---

## Docker

The `docker/cli/Dockerfile` image ships with `memvid-cli` pinned to the same version as this
crate. The version label (`org.opencontainers.image.version`) and the `npm install -g
memvid-cli@<version>` pin must stay in sync with `Cargo.toml`.

`scripts/check_version_consistency.py` enforces this automatically and is run in CI under the
`version-consistency` job.

---

## Benchmarks

```bash
cargo bench
```

Benchmarks are in `benches/`. They are informational only and not part of the correctness gate.
