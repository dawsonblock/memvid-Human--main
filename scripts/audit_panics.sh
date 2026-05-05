#!/usr/bin/env bash
# audit_panics.sh — Scan src/ for panic-family macros and unwrap/expect calls.
#
# Usage:
#   bash scripts/audit_panics.sh [--out DIR] [--allowlist FILE] [--strict]
#
# Options:
#   --out FILE       Write TSV report to this file (default: stdout)
#   --allowlist FILE TOML allowlist to cross-reference (default: tools/panic_allowlist.toml)
#   --strict         Exit 1 if any unreviewed production sites are found
#
# Output columns (TSV):
#   file  line  kind  snippet  classification
#
# classification values:
#   test        — inside a #[cfg(test)] block
#   allowlisted — matches an approved pattern in panic_allowlist.toml
#   review      — production site that needs manual review

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ALLOWLIST="${REPO_ROOT}/tools/panic_allowlist.toml"
OUT_FILE=""
STRICT=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out)        OUT_FILE="$2";   shift 2 ;;
    --allowlist)  ALLOWLIST="$2";  shift 2 ;;
    --strict)     STRICT=1;        shift   ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done

SRC="${REPO_ROOT}/src"

# ---------------------------------------------------------------------------
# Phase 1: collect all panic-family sites
# ---------------------------------------------------------------------------

TMPFILE="$(mktemp)"
trap 'rm -f "${TMPFILE}"' EXIT

# kinds: unwrap | expect | panic | unreachable | todo | unimplemented
grep -rn \
    -e '\.unwrap()' \
    -e '\.expect(' \
    -e 'panic!(' \
    -e 'unreachable!(' \
    -e 'todo!(' \
    -e 'unimplemented!(' \
    "${SRC}" --include="*.rs" \
  | sed 's|'"${SRC}/"'||' \
  > "${TMPFILE}" || true

TOTAL="$(wc -l < "${TMPFILE}" | tr -d ' ')"

# ---------------------------------------------------------------------------
# Phase 2: classify each site
# ---------------------------------------------------------------------------

# Build a list of the first #[cfg(test)] line for each file so we can
# heuristically classify lines that appear after that marker as test-only.
declare -A TEST_START
while IFS=: read -r file line _rest; do
  abs="${SRC}/${file}"
  if [[ ! -v "TEST_START[${file}]" ]]; then
    test_line="$(grep -n '#\[cfg(test)\]' "${abs}" 2>/dev/null | head -1 | cut -d: -f1 || true)"
    TEST_START["${file}"]="${test_line:-999999}"
  fi
done < "${TMPFILE}"

# Allowlisted snippet fragments (from tools/panic_allowlist.toml categories)
ALLOWLIST_PATTERNS=(
  'Regex::new('
  'try_into().unwrap()'
  'from_calendar_date('
  'from_ymd_opt('
  'timestamp_opt('
  'get_or_init('
  'unwrap_or_else(|_| unreachable!())'
  '\.expect("No default '
  'ConsolidationDisposition::Duplicate => unreachable!()'
)

classify() {
  local file="$1" lineno="$2" snippet="$3"
  local ts="${TEST_START[${file}]:-999999}"
  if (( lineno >= ts )); then
    echo "test"
    return
  fi
  for pat in "${ALLOWLIST_PATTERNS[@]}"; do
    if [[ "$snippet" == *"$pat"* ]]; then
      echo "allowlisted"
      return
    fi
  done
  echo "review"
}

# ---------------------------------------------------------------------------
# Phase 3: emit report
# ---------------------------------------------------------------------------

emit() {
  printf '%s\n' "file	line	kind	snippet	classification"
  while IFS=: read -r file lineno rest; do
    # detect kind
    kind="other"
    case "$rest" in
      *'.unwrap()'*)   kind="unwrap" ;;
      *'.expect('*)    kind="expect" ;;
      *'panic!('*)     kind="panic" ;;
      *'unreachable!('*) kind="unreachable" ;;
      *'todo!('*)      kind="todo" ;;
      *'unimplemented!('*) kind="unimplemented" ;;
    esac
    snippet="$(echo "$rest" | sed 's/^[[:space:]]*//' | cut -c1-80)"
    cls="$(classify "$file" "$lineno" "$snippet")"
    printf '%s\t%s\t%s\t%s\t%s\n' "$file" "$lineno" "$kind" "$snippet" "$cls"
  done < "${TMPFILE}"
}

if [[ -n "$OUT_FILE" ]]; then
  mkdir -p "$(dirname "$OUT_FILE")"
  emit > "$OUT_FILE"
  echo "Report written to: ${OUT_FILE}" >&2
else
  emit
fi

# ---------------------------------------------------------------------------
# Phase 4: summary and strict-mode exit code
# ---------------------------------------------------------------------------

REVIEW_COUNT="$(emit | tail -n +2 | awk -F'\t' '$5=="review"{c++} END{print c+0}')"
TEST_COUNT="$(emit   | tail -n +2 | awk -F'\t' '$5=="test"{c++} END{print c+0}')"
ALLOW_COUNT="$(emit  | tail -n +2 | awk -F'\t' '$5=="allowlisted"{c++} END{print c+0}')"

cat >&2 <<EOF

=== panic audit summary ===
Total sites   : ${TOTAL}
  test-only   : ${TEST_COUNT}
  allowlisted : ${ALLOW_COUNT}
  needs review: ${REVIEW_COUNT}
EOF

if [[ $STRICT -eq 1 && $REVIEW_COUNT -gt 0 ]]; then
  echo "ERROR: ${REVIEW_COUNT} unreviewed production panic site(s) found (--strict mode)" >&2
  exit 1
fi
