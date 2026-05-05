#!/usr/bin/env python3
"""test_audit_panics.py — pytest-compatible tests for audit_panics.py and panic_allowlist.toml.

Run with:
  python3.11 -m pytest scripts/test_audit_panics.py -v
or:
  python3.11 -m pytest scripts/test_audit_panics.py -v --tb=short

Requires Python 3.11+ (stdlib tomllib).
"""

import sys

if sys.version_info < (3, 11):
    import pytest  # noqa: E402

    pytest.skip(
        "Python 3.11+ required (stdlib tomllib); skipping all tests",
        allow_module_level=True,
    )

import subprocess
import tomllib
from pathlib import Path

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parent.parent
ALLOWLIST_PATH = REPO_ROOT / "tools" / "panic_allowlist.toml"
AUDIT_SCRIPT = REPO_ROOT / "scripts" / "audit_panics.py"
SRC_DIR = REPO_ROOT / "src"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _load_allowlist() -> dict:
    """Return raw TOML data from the allowlist, raising on failure."""
    return tomllib.loads(ALLOWLIST_PATH.read_text(encoding="utf-8"))


def _run_audit(*extra_args: str) -> subprocess.CompletedProcess:
    """Run audit_panics.py against the real src/ tree and return the result."""
    return subprocess.run(
        [sys.executable, str(AUDIT_SCRIPT), *extra_args],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
    )


# ---------------------------------------------------------------------------
# Allowlist structure tests
# ---------------------------------------------------------------------------


def test_allowlist_parses_without_error():
    """The TOML allowlist must parse cleanly."""
    data = _load_allowlist()
    assert "allow" in data, "TOML must contain a top-level [[allow]] array"
    assert isinstance(data["allow"], list), "[[allow]] must be a list"


def test_allowlist_required_fields():
    """Every entry must have id, kind, snippet_contains, and rationale."""
    data = _load_allowlist()
    required = {"id", "kind", "snippet_contains", "rationale"}
    for idx, entry in enumerate(data["allow"]):
        missing = required - entry.keys()
        assert (
            not missing
        ), f"Entry #{idx} (id={entry.get('id', '<missing>')!r}) is missing fields: {missing}"


def test_allowlist_kind_values_valid():
    """kind must be one of the recognised panic-family kinds or '*'."""
    valid_kinds = {
        "unwrap",
        "expect",
        "panic",
        "todo",
        "unimplemented",
        "unreachable",
        "*",
    }
    data = _load_allowlist()
    for entry in data["allow"]:
        assert (
            entry["kind"] in valid_kinds
        ), f"Entry {entry['id']!r} has unknown kind {entry['kind']!r}"


def test_allowlist_file_or_global():
    """Each entry must have a 'file' key or global=true (not neither)."""
    data = _load_allowlist()
    for entry in data["allow"]:
        has_file = bool(entry.get("file", ""))
        is_global = bool(entry.get("global", False))
        assert (
            has_file or is_global
        ), f"Entry {entry['id']!r} has neither 'file' nor global=true"


def test_allowlist_no_duplicate_ids():
    """All allowlist ids must be unique."""
    data = _load_allowlist()
    ids = [e["id"] for e in data["allow"]]
    seen: set[str] = set()
    duplicates: list[str] = []
    for entry_id in ids:
        if entry_id in seen:
            duplicates.append(entry_id)
        seen.add(entry_id)
    assert not duplicates, f"Duplicate allowlist ids found: {duplicates}"


def test_allowlist_minimum_entry_count():
    """There should be at least 40 entries (sanity-check against accidental truncation)."""
    data = _load_allowlist()
    count = len(data["allow"])
    assert count >= 40, f"Expected at least 40 allowlist entries; got {count}"


def test_allowlist_file_paths_exist():
    """Every non-global entry's 'file' path must exist under the repo root."""
    data = _load_allowlist()
    missing: list[str] = []
    for entry in data["allow"]:
        if entry.get("global", False):
            continue
        file_path = REPO_ROOT / entry["file"]
        if not file_path.exists():
            missing.append(entry["file"])
    assert not missing, f"Allowlist references non-existent files: {missing}"


# ---------------------------------------------------------------------------
# Audit script invocation tests
# ---------------------------------------------------------------------------


def test_audit_script_exists():
    """audit_panics.py must exist at scripts/audit_panics.py."""
    assert AUDIT_SCRIPT.exists(), f"Script not found: {AUDIT_SCRIPT}"


def test_audit_script_runs_without_error():
    """audit_panics.py must exit 0 in normal (non-strict) mode."""
    result = _run_audit("--format", "tsv")
    assert result.returncode == 0, (
        f"audit_panics.py exited {result.returncode}\n"
        f"stderr: {result.stderr[:2000]}"
    )


def test_audit_strict_mode_passes():
    """--strict must exit 0 with the current allowlist (review=0)."""
    result = _run_audit("--strict", "--format", "tsv")
    assert result.returncode == 0, (
        f"--strict mode exited {result.returncode} (unallowlisted production panics found)\n"
        f"stdout: {result.stdout[:2000]}\n"
        f"stderr: {result.stderr[:2000]}"
    )


def test_audit_total_count_in_expected_range():
    """Total panic sites should be within a reasonable range around the known baseline."""
    result = _run_audit("--format", "tsv")
    assert result.returncode == 0

    # TSV has a header line; count data rows.
    lines = [
        ln
        for ln in result.stdout.splitlines()
        if ln.strip() and not ln.startswith("file\t")
    ]
    total = len(lines)
    assert total >= 600, f"Total findings unexpectedly low: {total} (expected ≥ 600)"
    assert total <= 900, f"Total findings unexpectedly high: {total} (expected ≤ 900)"


def test_audit_test_count_plausible():
    """Test-classified sites should dominate (≥ 400) given the large test suite."""
    result = _run_audit("--format", "tsv")
    assert result.returncode == 0

    test_count = sum(1 for ln in result.stdout.splitlines() if ln.endswith("\ttest"))
    assert (
        test_count >= 400
    ), f"Unexpectedly few test-classified sites: {test_count} (expected ≥ 400)"


def test_audit_review_count_zero():
    """Production review sites must be 0 (strict mode must pass)."""
    result = _run_audit("--format", "tsv")
    assert result.returncode == 0

    review_count = sum(
        1 for ln in result.stdout.splitlines() if ln.endswith("\treview")
    )
    assert review_count == 0, (
        f"{review_count} unallowlisted production panic site(s) found — "
        f"run 'python3 scripts/audit_panics.py --strict' and update the allowlist."
    )


def test_audit_json_format():
    """--format json must produce valid JSON with the expected structure."""
    import json

    result = _run_audit("--format", "json")
    assert result.returncode == 0, f"JSON format exited {result.returncode}"

    data = json.loads(result.stdout)
    assert isinstance(data, list), "JSON output must be a top-level array"
    if data:
        first = data[0]
        for field in ("file", "line", "kind", "snippet", "classification"):
            assert field in first, f"JSON entry missing field {field!r}"


def test_audit_tsv_columns():
    """TSV output must have exactly 5 tab-separated columns per data row."""
    result = _run_audit("--format", "tsv")
    assert result.returncode == 0

    data_lines = [
        ln
        for ln in result.stdout.splitlines()
        if ln.strip() and not ln.startswith("file\t")
    ]
    assert data_lines, "No TSV data rows produced"

    for ln in data_lines[:50]:  # sample first 50
        parts = ln.split("\t")
        assert (
            len(parts) == 5
        ), f"Expected 5 TSV columns, got {len(parts)}: {ln[:120]!r}"


def test_audit_custom_allowlist_path(tmp_path):
    """--allowlist flag must accept an alternate path."""
    # Write a minimal allowlist with zero entries.
    minimal = "[meta]\nversion = 1\n\n"
    alt_path = tmp_path / "empty_allowlist.toml"
    alt_path.write_text(minimal, encoding="utf-8")

    result = _run_audit("--allowlist", str(alt_path), "--format", "tsv")
    # Expect exit 0 (not strict mode) even with empty allowlist.
    assert (
        result.returncode == 0
    ), f"Custom allowlist flag failed: exit {result.returncode}\n{result.stderr[:1000]}"


def test_audit_out_flag(tmp_path):
    """--out FILE must write the report to the specified path."""
    out_file = tmp_path / "report.tsv"
    result = _run_audit("--out", str(out_file), "--format", "tsv")
    assert result.returncode == 0
    assert out_file.exists(), "--out file was not created"
    content = out_file.read_text(encoding="utf-8")
    assert len(content) > 0, "--out file is empty"
