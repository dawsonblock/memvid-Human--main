#!/usr/bin/env bash
# audit_panics.sh — thin wrapper; real logic lives in audit_panics.py.
#
# Usage:
#   bash scripts/audit_panics.sh [--out FILE] [--allowlist FILE] [--strict] [--format tsv|json]
#
# All arguments are forwarded verbatim to audit_panics.py.
# Requires Python 3.11+ (stdlib tomllib).

set -euo pipefail
exec python3 "$(dirname "$0")/audit_panics.py" "$@"
