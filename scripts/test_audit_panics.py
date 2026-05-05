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
import importlib.util as _ilu

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parent.parent
ALLOWLIST_PATH = REPO_ROOT / "tools" / "panic_allowlist.toml"
AUDIT_SCRIPT = REPO_ROOT / "scripts" / "audit_panics.py"
SRC_DIR = REPO_ROOT / "src"

# ---------------------------------------------------------------------------
# Import audit_panics as a module (safe: guarded by if __name__=="__main__")
# ---------------------------------------------------------------------------

_spec = _ilu.spec_from_file_location("audit_panics", AUDIT_SCRIPT)
_audit_mod = _ilu.module_from_spec(_spec)
_spec.loader.exec_module(_audit_mod)  # type: ignore[union-attr]

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
# Module-scope fixture: run the full repo scan once, cache for all tests
# ---------------------------------------------------------------------------

import pytest


@pytest.fixture(scope="module")
def real_scan_findings():
    """Run the full repo scan once per test session; cache results."""
    findings, _ = _audit_mod.scan(SRC_DIR, ALLOWLIST_PATH)
    return findings


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


def test_audit_total_count_in_expected_range(real_scan_findings):
    """Total panic sites should be within a reasonable range around the known baseline."""
    total = len(real_scan_findings)
    assert total >= 600, f"Total findings unexpectedly low: {total} (expected ≥ 600)"
    assert total <= 900, f"Total findings unexpectedly high: {total} (expected ≤ 900)"


def test_audit_test_count_plausible(real_scan_findings):
    """Test-classified sites should dominate (≥ 400) given the large test suite."""
    test_count = sum(1 for f in real_scan_findings if f["classification"] == "test")
    assert (
        test_count >= 400
    ), f"Unexpectedly few test-classified sites: {test_count} (expected ≥ 400)"


def test_audit_review_count_zero(real_scan_findings):
    """Production review sites must be 0 (strict mode must pass)."""
    review_count = sum(1 for f in real_scan_findings if f["classification"] == "review")
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


# ---------------------------------------------------------------------------
# Isolated regression tests — use tmp_path, no full-repo subprocess
# ---------------------------------------------------------------------------


def test_isolated_production_unwrap_fails_strict(tmp_path):
    """A bare production unwrap with no allowlist must be flagged as 'review'."""
    src = tmp_path / "src"
    src.mkdir()
    (src / "lib.rs").write_text(
        "pub fn foo() { let x: Option<i32> = None; x.unwrap(); }\n",
        encoding="utf-8",
    )
    allowlist = tmp_path / "allow.toml"
    allowlist.write_text("[meta]\nversion = 1\n\n", encoding="utf-8")
    findings, _ = _audit_mod.scan(src, allowlist)
    review = [f for f in findings if f["classification"] == "review"]
    assert len(review) >= 1, f"Expected at least 1 review finding, got: {findings}"


def test_isolated_allowlist_entry_makes_it_pass(tmp_path):
    """An allowlisted unwrap must be classified as 'allowlisted', not 'review'."""
    src = tmp_path / "src"
    src.mkdir()
    (src / "lib.rs").write_text(
        "pub fn foo() { let x: Option<i32> = None; x.unwrap(); }\n",
        encoding="utf-8",
    )
    allowlist = tmp_path / "allow.toml"
    allowlist.write_text(
        "[meta]\nversion = 1\n\n"
        "[[allow]]\n"
        'id = "test-entry"\n'
        'file = "src/lib.rs"\n'
        'kind = "unwrap"\n'
        'snippet_contains = "x.unwrap()"\n'
        'rationale = "test"\n'
        'remediation = "n/a"\n',
        encoding="utf-8",
    )
    findings, _ = _audit_mod.scan(src, allowlist)
    review = [f for f in findings if f["classification"] == "review"]
    assert (
        len(review) == 0
    ), f"Expected 0 review findings after allowlisting, got: {review}"


def test_isolated_dynamic_regex_not_allowlisted(tmp_path):
    """Regex::new(var).unwrap() (dynamic) must NOT be allowlisted by the real allowlist."""
    src = tmp_path / "src"
    src.mkdir()
    # Dynamic pattern — NOT a raw-string literal
    (src / "lib.rs").write_text(
        "use regex::Regex;\npub fn search(pat: &str) { Regex::new(pat).unwrap(); }\n",
        encoding="utf-8",
    )
    findings, _ = _audit_mod.scan(src, ALLOWLIST_PATH)
    review = [f for f in findings if f["classification"] == "review"]
    assert len(review) >= 1, (
        "Dynamic Regex::new(var).unwrap() must be flagged as review — "
        'the per-file entries only match raw-string literals (Regex::new(r"..."))'
    )


def test_isolated_test_code_classified_as_test(tmp_path):
    """unwrap() inside a #[cfg(test)] block must be classified as 'test', not 'review'."""
    src = tmp_path / "src"
    src.mkdir()
    (src / "lib.rs").write_text(
        "#[cfg(test)]\nmod tests {\n"
        "    fn it_works() { let x: Option<i32> = None; x.unwrap(); }\n"
        "}\n",
        encoding="utf-8",
    )
    allowlist = tmp_path / "allow.toml"
    allowlist.write_text("[meta]\nversion = 1\n\n", encoding="utf-8")
    findings, _ = _audit_mod.scan(src, allowlist)
    review = [f for f in findings if f["classification"] == "review"]
    test_cls = [f for f in findings if f["classification"] == "test"]
    assert len(review) == 0, f"Test-scope unwrap must not be 'review': {review}"
    assert (
        len(test_cls) >= 1
    ), f"Test-scope unwrap must be classified 'test': {findings}"


def test_isolated_malformed_toml_fails(tmp_path):
    """A malformed allowlist TOML must cause the audit to exit non-zero."""
    src = tmp_path / "src"
    src.mkdir()
    (src / "lib.rs").write_text("pub fn foo() {}\n", encoding="utf-8")
    bad_allowlist = tmp_path / "bad.toml"
    bad_allowlist.write_text("this is not valid toml ][[\n", encoding="utf-8")
    result = subprocess.run(
        [
            sys.executable,
            str(AUDIT_SCRIPT),
            "--allowlist",
            str(bad_allowlist),
            "--src",
            str(src),
        ],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
    )
    assert result.returncode != 0, "Malformed TOML allowlist must cause non-zero exit"


def test_isolated_malformed_toml_direct(tmp_path):
    """_parse_allowlist() must raise SystemExit on malformed TOML."""
    bad = tmp_path / "bad.toml"
    bad.write_text("this is not valid toml ][[\n", encoding="utf-8")
    with pytest.raises(SystemExit):
        _audit_mod._parse_allowlist(bad)


def test_isolated_prod_after_test_module_is_review(tmp_path):
    """Production unwrap after a test module must be classified 'review', not 'test'."""
    src = tmp_path / "src"
    src.mkdir()
    (src / "lib.rs").write_text(
        "#[cfg(test)]\nmod tests {\n"
        "    fn t() { let _: Option<i32> = None; let _ = Some(1).unwrap(); }\n"
        "}\n"
        "pub fn prod() { let _ = Some(2).unwrap(); }\n",
        encoding="utf-8",
    )
    allowlist = tmp_path / "allow.toml"
    allowlist.write_text("[meta]\nversion = 1\n\n", encoding="utf-8")
    findings, _ = _audit_mod.scan(src, allowlist)
    test_cls = [f for f in findings if f["classification"] == "test"]
    review_cls = [f for f in findings if f["classification"] == "review"]
    assert (
        len(test_cls) >= 1
    ), f"Expected test finding inside cfg(test) block: {findings}"
    assert (
        len(review_cls) >= 1
    ), f"Expected review finding for prod() after test module: {findings}"


def test_isolated_multiple_panics_per_line(tmp_path):
    """Both unwrap and panic! on the same line must both be reported."""
    src = tmp_path / "src"
    src.mkdir()
    (src / "lib.rs").write_text(
        'pub fn foo() { Some(1).unwrap(); panic!("x"); }\n',
        encoding="utf-8",
    )
    allowlist = tmp_path / "allow.toml"
    allowlist.write_text("[meta]\nversion = 1\n\n", encoding="utf-8")
    findings, _ = _audit_mod.scan(src, allowlist)
    assert (
        len(findings) >= 2
    ), f"Expected at least 2 findings (unwrap + panic), got: {findings}"
    line_nums = {f["line"] for f in findings}
    assert len(line_nums) == 1, f"Both findings must be on the same line: {findings}"


def test_isolated_cfg_test_non_braced_item_does_not_bleed(tmp_path):
    """#[cfg(test)] on a non-braced item must not classify the next production item as test."""
    src = tmp_path / "src"
    src.mkdir()
    (src / "lib.rs").write_text(
        "#[cfg(test)] const X: i32 = 1;\n" "pub fn prod() { Some(1).unwrap(); }\n",
        encoding="utf-8",
    )
    allowlist = tmp_path / "allow.toml"
    allowlist.write_text("[meta]\nversion = 1\n\n", encoding="utf-8")
    findings, _ = _audit_mod.scan(src, allowlist)
    review_cls = [f for f in findings if f["classification"] == "review"]
    assert (
        len(review_cls) >= 1
    ), f"prod() unwrap after non-braced cfg(test) must be 'review', not 'test': {findings}"
