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

## Panic Audit

`scripts/audit_panics.py` scans `src/**/*.rs` for panic-family calls
(`unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`)
and classifies each site as **test**, **allowlisted**, or **review**.

```bash
# Informational run — always exits 0, prints summary
python3 scripts/audit_panics.py

# Strict mode — exits 1 if any review-class site exists
python3 scripts/audit_panics.py --strict

# Write a TSV report to disk
python3 scripts/audit_panics.py --out artifacts/audits/panic_report.tsv
```

Approved production sites are recorded in `tools/panic_allowlist.toml`
(50 entries as of v2.0.139).  Add new entries there whenever a
legitimate panic site would otherwise block `--strict`.

The `panic-audit` CI job runs `--strict` on every push and uploads
`panic_report.tsv` as a GitHub Actions artifact.

> **Requires:** Python 3.11+ (uses stdlib `tomllib`).

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

## Clean-Clone Proof

`scripts/proof_clean_clone.sh` proves the crate builds and all tests pass starting from a fresh
local clone — simulating a first-checkout environment.

> **Requires:** A real git checkout with a `.git` directory.  This script uses `git clone --local`
> internally; it will fail on zip exports or directories without `.git`.

```bash
bash scripts/proof_clean_clone.sh
# optionally: bash scripts/proof_clean_clone.sh --out /tmp/myproof
```

Log files are written to `artifacts/proof/` with the naming convention
`clean_clone-<YYYYMMDDTHHMMSSZ>.log`.

Steps performed:
1. Clones the repo into a temporary directory using `git clone --local`
2. Runs `cargo fmt --check`
3. Runs `cargo clippy --all-targets`
4. Runs `cargo test --features "lex,pdf_extract,simd"` (default profile)
5. Runs `cargo test --no-default-features` (minimal kernel)
6. Cleans up the temporary clone on exit

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
    AuditLogger::new(clock.clone(), Arc::new(InMemoryAuditSink::default())),
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
