#!/usr/bin/env bash
# proof_baseline.sh — Run all verifiable test profiles and write timestamped logs.
#
# Usage:
#   bash scripts/proof_baseline.sh [--out DIR]
#
# Options:
#   --out DIR   Directory for log output (default: artifacts/proof)
#
# Skips profiles that require optional native libraries (onnx/vec) which are
# not available in the standard CI environment.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${REPO_ROOT}/artifacts/proof"

# --- argument parsing ---
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) OUT_DIR="$2"; shift 2 ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done

mkdir -p "${OUT_DIR}"

TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
LOG="${OUT_DIR}/baseline-${TIMESTAMP}.log"

log() { echo "[$(date -u +%H:%M:%SZ)] $*" | tee -a "${LOG}"; }

log "=== proof_baseline.sh ==="
log "repo:      ${REPO_ROOT}"
log "out dir:   ${OUT_DIR}"
log "log file:  ${LOG}"
log ""

# --- version banner ---
log "--- toolchain versions ---"
rustc --version 2>&1 | tee -a "${LOG}"
cargo --version 2>&1 | tee -a "${LOG}"
uname -a 2>&1 | tee -a "${LOG}" || true
cargo metadata --locked --no-deps --format-version 1 2>&1 | head -5 | tee -a "${LOG}"
log ""

# --- fmt + lint ---
log "--- fmt check ---"
cargo fmt --all -- --check 2>&1 | tee -a "${LOG}"
log "fmt: PASS"
log ""
log "--- clippy ---"
cargo clippy --features "lex,pdf_extract,simd" -- -D warnings -A clippy::non_std_lazy_statics \
  2>&1 | tee -a "${LOG}"
log "clippy: PASS"
log ""

# --- profile 1: default feature set ---
log "--- profile: default (lex,pdf_extract,simd) ---"
cargo test --features "lex,pdf_extract,simd" \
  2>&1 | tee -a "${LOG}"
log "profile default: PASS"
log ""

# --- profile 2: minimal kernel (no default features) ---
log "--- profile: minimal kernel (no-default-features) ---"
cargo test --no-default-features \
  2>&1 | tee -a "${LOG}"
log "profile minimal: PASS"
log ""

# --- profile 3: MSRV smoke (1.85.0) ---
log "--- profile: MSRV build + test (rustup 1.85.0) ---"
rustup install 1.85.0 --no-self-update 2>&1 | tee -a "${LOG}"
rustup run 1.85.0 cargo build --features "lex,pdf_extract,simd" \
  2>&1 | tee -a "${LOG}"
rustup run 1.85.0 cargo test --features "lex,pdf_extract,simd" \
  2>&1 | tee -a "${LOG}"
log "profile MSRV build+test: PASS"
log ""

log "=== all profiles passed ==="
log "log written to: ${LOG}"
