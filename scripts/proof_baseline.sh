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

# --- profile 3: MSRV smoke (default features, current stable proxy) ---
log "--- profile: MSRV build smoke (default features) ---"
cargo build --features "lex,pdf_extract,simd" \
  2>&1 | tee -a "${LOG}"
log "profile MSRV build: PASS"
log ""

log "=== all profiles passed ==="
log "log written to: ${LOG}"
