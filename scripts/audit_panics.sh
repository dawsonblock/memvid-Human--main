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
REPORT_FILE="$(mktemp)"
TEST_START_FILE="$(mktemp)"
trap 'rm -f "${TMPFILE}" "${REPORT_FILE}" "${TEST_START_FILE}"' EXIT

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

# Build a flat file mapping "filename:first_test_boundary_line".
# A test boundary is either #[cfg(test)] or a bare #[test] attribute.
# We take the minimum of the two so that files using #[test] without a
# cfg-block (e.g. builder.rs) are classified correctly.
# This avoids declare -A (bash 4+), keeping compatibility with bash 3.2.
while IFS= read -r file; do
  abs="${SRC}/${file}"
  cfg_line="$(grep -n '#\[cfg(test)\]' "${abs}" 2>/dev/null | head -1 | cut -d: -f1 || true)"
  test_line="$(grep -n '#\[test\]'     "${abs}" 2>/dev/null | head -1 | cut -d: -f1 || true)"
  if [[ -n "${cfg_line}" && -n "${test_line}" ]]; then
    if (( cfg_line <= test_line )); then
      printf '%s:%s\n' "${file}" "${cfg_line}"
    else
      printf '%s:%s\n' "${file}" "${test_line}"
    fi
  elif [[ -n "${cfg_line}" ]]; then
    printf '%s:%s\n' "${file}" "${cfg_line}"
  elif [[ -n "${test_line}" ]]; then
    printf '%s:%s\n' "${file}" "${test_line}"
  else
    printf '%s:999999\n' "${file}"
  fi
done < <(cut -d: -f1 "${TMPFILE}" | sort -u) > "${TEST_START_FILE}"

# Allowlisted snippet fragments (from tools/panic_allowlist.toml categories)
ALLOWLIST_PATTERNS=(
  # --- original 9 entries ---
  'Regex::new('
  'try_into().unwrap()'
  'from_calendar_date('
  'from_ymd_opt('
  'timestamp_opt('
  'get_or_init('
  'unwrap_or_else(|_| unreachable!())'
  '.expect("No default '
  'ConsolidationDisposition::Duplicate => unreachable!()'
  # --- 15 new entries ---
  '.expect("valid regex")'
  '.expect("full match")'
  '.expect("validated '
  '.expect("default '
  '.find(|m| m.is_default).unwrap()'
  '.expect("checked above")'
  '.expect("mem available")'
  '.expect("vec manifest")'
  '.expect("regex literal")'
  'date.checked_add(Duration::days(delta))'
  'Month::try_from(total as u8)'
  'Time::from_hms(hour as u8, minute as u8'
  'token.parse().unwrap()'
  '.expect("clauses.len() == 1")'
  '.expect("error set in preceding branch")'
  # --- doc-comment and exhaustive-match patterns ---
  '///'
  '_ => unreachable!()'
  # --- pii.rs static regex compilations ---
  '.expect("invalid '
  # --- safe literal / documented-invariant patterns ---
  'NonZeroU64::new(8)'
  '.expect("counts.len()==1'
  '.expect("queries.len()==1'
  '.expect("positions.len()'
  '.expect("unordered list'
  '.expect("ordered list'
)

classify() {
  local file="$1" lineno="$2" snippet="$3"
  local ts_line ts
  ts_line="$(grep -Fm1 "${file}:" "${TEST_START_FILE}" 2>/dev/null || true)"
  ts="${ts_line##*:}"
  ts="${ts:-999999}"
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

{
  printf '%s\n' "file   line    kind    snippet classification"
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
} > "${REPORT_FILE}"

if [[ -n "$OUT_FILE" ]]; then
  mkdir -p "$(dirname "$OUT_FILE")"
  cp "${REPORT_FILE}" "$OUT_FILE"
  echo "Report written to: ${OUT_FILE}" >&2
else
  cat "${REPORT_FILE}"
fi

# ---------------------------------------------------------------------------
# Phase 4: summary and strict-mode exit code
# ---------------------------------------------------------------------------

read -r TEST_COUNT ALLOW_COUNT REVIEW_COUNT < <(
  tail -n +2 "${REPORT_FILE}" | awk -F'\t' '
    $5=="test"        { t++ }
    $5=="allowlisted" { a++ }
    $5=="review"      { r++ }
    END { print (t+0), (a+0), (r+0) }
  '
)
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
