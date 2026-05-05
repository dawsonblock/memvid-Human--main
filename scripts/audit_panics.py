#!/usr/bin/env python3
"""audit_panics.py — Scan src/**/*.rs for panic-family macros and classify them.

Usage:
  python3 scripts/audit_panics.py [--strict] [--out FILE] [--allowlist FILE] [--format tsv|json]

Options:
  --strict          Exit 1 if any 'review' finding exists or if the allowlist fails to parse.
  --out FILE        Write report to FILE instead of stdout.
  --allowlist FILE  TOML allowlist path (default: tools/panic_allowlist.toml).
  --format tsv|json Output format (default: tsv).

Output columns (TSV):
  file  line  kind  snippet  classification

Classification values:
  test        — line is inside or after the first #[cfg(test)] / #[test] boundary in the file
  allowlisted — matches an approved entry in the allowlist TOML
  review      — production site that requires manual review

Note: Only the FIRST matching panic-family kind on each line is reported.
      Lines whose non-whitespace content starts with '//' are skipped (comments).

Requires Python 3.11+.
"""

import sys

if sys.version_info < (3, 11):
    print(
        f"ERROR: Python 3.11+ required (stdlib tomllib); got {sys.version}",
        file=sys.stderr,
    )
    sys.exit(1)

import argparse
import json
import re
import tomllib
from pathlib import Path

# ---------------------------------------------------------------------------
# Pattern definitions — checked in order; first match per line wins.
# ---------------------------------------------------------------------------
_PATTERNS: list[tuple[str, re.Pattern[str]]] = [
    ("unwrap", re.compile(r"\.unwrap\s*\(\s*\)")),
    ("expect", re.compile(r"\.expect\s*\(")),
    ("panic", re.compile(r"\bpanic!\s*\(")),
    ("todo", re.compile(r"\btodo!\s*\(")),
    ("unimplemented", re.compile(r"\bunimplemented!\s*\(")),
    ("unreachable", re.compile(r"\bunreachable!\s*\(")),
]

# A line is a comment (and therefore skipped) when the first non-whitespace
# characters are "//".  This covers line comments, doc comments (///), and
# module-level doc comments (//!).
_COMMENT_RE = re.compile(r"^\s*//")

# Test-boundary markers: when either pattern matches a line, every subsequent
# line in the same file is classified as "test".
_TEST_BOUNDARY_RE = re.compile(
    r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]" r"|" r"#\s*\[\s*test\s*\]"
)


# ---------------------------------------------------------------------------
# Data classes (plain dicts OK but named tuples improve readability)
# ---------------------------------------------------------------------------
from typing import TypedDict


class AllowEntry(TypedDict):
    id: str
    kind: str  # "unwrap"|"expect"|"panic"|"todo"|"unimplemented"|"unreachable"|"*"
    snippet_contains: str  # literal substring
    file: str  # relative path; empty string when global=True
    global_: bool  # True → file check skipped


class Finding(TypedDict):
    file: str  # repo-relative, forward-slash
    line: int  # 1-based
    kind: str
    snippet: str  # raw source line, stripped of trailing newline
    classification: str  # "test" | "allowlisted" | "review"


# ---------------------------------------------------------------------------
# Allowlist parsing
# ---------------------------------------------------------------------------


def _parse_allowlist(path: Path) -> list[AllowEntry]:
    """Parse tools/panic_allowlist.toml and return a list of AllowEntry objects.

    Raises SystemExit(1) with a descriptive message on any parse/validation error.
    """
    try:
        raw = tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        print(f"ERROR: allowlist not found: {path}", file=sys.stderr)
        sys.exit(1)
    except tomllib.TOMLDecodeError as exc:
        print(f"ERROR: allowlist TOML parse failed: {exc}", file=sys.stderr)
        sys.exit(1)

    entries: list[AllowEntry] = []
    allow_list = raw.get("allow", [])
    if not isinstance(allow_list, list):
        print(
            "ERROR: allowlist TOML must have a top-level [[allow]] array",
            file=sys.stderr,
        )
        sys.exit(1)

    seen_ids: set[str] = set()
    for idx, item in enumerate(allow_list):
        # Validate required fields
        for field in ("id", "kind", "snippet_contains"):
            if field not in item:
                print(
                    f"ERROR: allowlist entry #{idx} missing required field '{field}'",
                    file=sys.stderr,
                )
                sys.exit(1)

        entry_id = item["id"]
        if entry_id in seen_ids:
            print(f"ERROR: duplicate allowlist id '{entry_id}'", file=sys.stderr)
            sys.exit(1)
        seen_ids.add(entry_id)

        is_global = bool(item.get("global", False))
        file_val = item.get("file", "")
        if not is_global and not file_val:
            print(
                f"ERROR: allowlist entry '{entry_id}' requires either "
                f"global=true or a 'file' field",
                file=sys.stderr,
            )
            sys.exit(1)

        entries.append(
            AllowEntry(
                id=entry_id,
                kind=item["kind"],
                snippet_contains=item["snippet_contains"],
                file=file_val,
                global_=is_global,
            )
        )

    return entries


def _is_allowlisted(
    rel_file: str,
    kind: str,
    line_text: str,
    entries: list[AllowEntry],
) -> bool:
    for entry in entries:
        # Kind check
        if entry["kind"] != "*" and entry["kind"] != kind:
            continue
        # Snippet check
        if entry["snippet_contains"] not in line_text:
            continue
        # File check
        if entry["global_"]:
            return True
        if entry["file"] == rel_file:
            return True
    return False


# ---------------------------------------------------------------------------
# Scanner
# ---------------------------------------------------------------------------


def _scan_file(
    src_root: Path,
    rs_file: Path,
    entries: list[AllowEntry],
) -> list[Finding]:
    """Scan a single .rs file and return all findings."""
    rel = rs_file.relative_to(src_root.parent).as_posix()  # e.g. src/foo/bar.rs

    try:
        lines = rs_file.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError as exc:
        print(f"WARNING: could not read {rs_file}: {exc}", file=sys.stderr)
        return []

    # Determine the first test-boundary line (0-based index).
    test_boundary: int | None = None
    for i, line in enumerate(lines):
        if _TEST_BOUNDARY_RE.search(line):
            test_boundary = i
            break

    findings: list[Finding] = []

    for i, line in enumerate(lines):
        # Skip comment lines
        if _COMMENT_RE.match(line):
            continue

        # Check each pattern (first match wins)
        matched_kind: str | None = None
        for kind, pat in _PATTERNS:
            if pat.search(line):
                matched_kind = kind
                break

        if matched_kind is None:
            continue

        # Classify
        if test_boundary is not None and i >= test_boundary:
            classification = "test"
        elif _is_allowlisted(rel, matched_kind, line, entries):
            classification = "allowlisted"
        else:
            classification = "review"

        findings.append(
            Finding(
                file=rel,
                line=i + 1,
                kind=matched_kind,
                snippet=line.rstrip("\r\n"),
                classification=classification,
            )
        )

    return findings


def scan(
    src_root: Path,
    allowlist_path: Path,
) -> tuple[list[Finding], list[AllowEntry]]:
    """Scan src_root/**/*.rs against allowlist_path. Returns (findings, entries)."""
    entries = _parse_allowlist(allowlist_path)

    all_findings: list[Finding] = []
    for rs_file in sorted(src_root.rglob("*.rs")):
        all_findings.extend(_scan_file(src_root, rs_file, entries))

    return all_findings, entries


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------


def _write_tsv(findings: list[Finding], out) -> None:
    out.write("file\tline\tkind\tsnippet\tclassification\n")
    for f in findings:
        snippet = f["snippet"].replace("\t", "\\t")
        out.write(
            f"{f['file']}\t{f['line']}\t{f['kind']}\t{snippet}\t{f['classification']}\n"
        )


def _write_json(findings: list[Finding], out) -> None:
    json.dump(findings, out, indent=2)
    out.write("\n")


def _print_summary(findings: list[Finding]) -> None:
    total = len(findings)
    test = sum(1 for f in findings if f["classification"] == "test")
    allowed = sum(1 for f in findings if f["classification"] == "allowlisted")
    review = sum(1 for f in findings if f["classification"] == "review")
    print(
        f"\nPanic audit summary: total={total}  test={test}  "
        f"allowlisted={allowed}  review={review}",
        file=sys.stderr,
    )


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument(
        "--strict", action="store_true", help="Exit 1 if any review findings"
    )
    p.add_argument(
        "--out", metavar="FILE", help="Write report to FILE (default: stdout)"
    )
    p.add_argument(
        "--allowlist",
        metavar="FILE",
        help="TOML allowlist (default: tools/panic_allowlist.toml)",
    )
    p.add_argument(
        "--format",
        choices=["tsv", "json"],
        default="tsv",
        help="Output format (default: tsv)",
    )
    return p


def main() -> None:
    args = _build_parser().parse_args()

    repo_root = Path(__file__).resolve().parent.parent
    src_root = repo_root / "src"

    allowlist_path = (
        Path(args.allowlist)
        if args.allowlist
        else repo_root / "tools" / "panic_allowlist.toml"
    )
    if not allowlist_path.is_absolute():
        allowlist_path = repo_root / allowlist_path

    findings, _entries = scan(src_root, allowlist_path)

    _print_summary(findings)

    # Write report
    if args.out:
        out_path = Path(args.out)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        with out_path.open("w", encoding="utf-8") as fh:
            if args.format == "json":
                _write_json(findings, fh)
            else:
                _write_tsv(findings, fh)
    else:
        if args.format == "json":
            _write_json(findings, sys.stdout)
        else:
            _write_tsv(findings, sys.stdout)

    # Strict mode: exit 1 if any review findings
    if args.strict:
        review_count = sum(1 for f in findings if f["classification"] == "review")
        if review_count > 0:
            print(
                f"STRICT MODE: {review_count} unallowlisted production panic site(s) found. "
                f"Add per-file entries to tools/panic_allowlist.toml or fix the source.",
                file=sys.stderr,
            )
            sys.exit(1)


if __name__ == "__main__":
    main()
