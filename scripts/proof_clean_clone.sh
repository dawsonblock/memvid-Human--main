#!/usr/bin/env bash
# proof_clean_clone.sh — Prove the crate builds and tests pass from a clean local clone.
#
# Clones the repo locally into a temp directory to simulate a first-checkout environment,
# then runs fmt, clippy (--all-targets), default feature tests, and minimal kernel tests.
# Writes a timestamped log to artifacts/proof/ in the original repo.
#
# Usage:
#   bash scripts/proof_clean_clone.sh [--out DIR]
#
# Options:
#   --out DIR   Directory for log output (default: artifacts/proof)

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
LOG="${OUT_DIR}/clean_clone-${TIMESTAMP}.log"

log() { echo "[$(date -u +%H:%M:%SZ)] $*" | tee -a "${LOG}"; }

# --- temp clone with cleanup trap ---
TMP_CLONE="$(mktemp -d)"
cleanup() { rm -rf "${TMP_CLONE}"; }
trap cleanup EXIT

log "=== proof_clean_clone.sh ==="
log "repo:        ${REPO_ROOT}"
log "clone dir:   ${TMP_CLONE}"
log "out dir:     ${OUT_DIR}"
log "log file:    ${LOG}"
log ""

# --- toolchain banner ---
log "--- toolchain versions ---"
rustc --version 2>&1 | tee -a "${LOG}"
cargo --version 2>&1 | tee -a "${LOG}"
uname -a 2>&1 | tee -a "${LOG}" || true
log ""

# --- clone ---
log "--- cloning (--local) ---"
git clone --local "${REPO_ROOT}" "${TMP_CLONE}" 2>&1 | tee -a "${LOG}"
log "clone: OK"
log ""

cd "${TMP_CLONE}"

# --- fmt check ---
log "--- fmt check ---"
cargo fmt --all -- --check 2>&1 | tee -a "${LOG}"
log "fmt: PASS"
log ""

# --- clippy ---
log "--- clippy (--all-targets) ---"
cargo clippy --all-targets --features "lex,pdf_extract,simd" -- -D warnings -A clippy::non_std_lazy_statics \
  2>&1 | tee -a "${LOG}"
log "clippy: PASS"
log ""

# --- profile 1: default feature set ---
log "--- profile: default (lex,pdf_extract,simd) ---"
cargo test --features "lex,pdf_extract,simd" \
  2>&1 | tee -a "${LOG}"
log "profile default: PASS"
log ""

# --- profile 2: minimal kernel ---
log "--- profile: minimal kernel (no-default-features) ---"
cargo test --no-default-features \
  2>&1 | tee -a "${LOG}"
log "profile minimal: PASS"
log ""

log "=== all clean-clone checks passed ==="
log "log written to: ${LOG}"
