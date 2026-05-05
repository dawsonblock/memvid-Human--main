#!/usr/bin/env python3
"""
check_version_consistency.py — Verify that the Cargo.toml crate version, the Docker image label,
and the npm pin in docker/cli/Dockerfile all agree.

Exit code 0 → all consistent.
Exit code 1 → mismatch found (details printed to stderr).
"""

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# ── sources ──────────────────────────────────────────────────────────────────


def cargo_version() -> str:
    text = (REPO_ROOT / "Cargo.toml").read_text()
    m = re.search(r'^version\s*=\s*"([^"]+)"', text, re.MULTILINE)
    if not m:
        raise ValueError("Could not find version in Cargo.toml")
    return m.group(1)


def dockerfile_label_version(path: Path) -> str:
    text = path.read_text()
    m = re.search(r'org\.opencontainers\.image\.version\s*=\s*"([^"]+)"', text)
    if not m:
        raise ValueError(
            f"Could not find org.opencontainers.image.version label in {path}"
        )
    return m.group(1)


def dockerfile_npm_pin(path: Path) -> str:
    text = path.read_text()
    m = re.search(r"npm install -g memvid-cli@([^\s\\]+)", text)
    if not m:
        raise ValueError(f"Could not find memvid-cli npm pin in {path}")
    return m.group(1)


# ── main ──────────────────────────────────────────────────────────────────────


def main() -> int:
    dockerfile = REPO_ROOT / "docker" / "cli" / "Dockerfile"

    try:
        cargo_ver = cargo_version()
        label_ver = dockerfile_label_version(dockerfile)
        npm_pin = dockerfile_npm_pin(dockerfile)
    except (ValueError, FileNotFoundError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1

    sources = {
        "Cargo.toml": cargo_ver,
        "docker/cli/Dockerfile (label)": label_ver,
        "docker/cli/Dockerfile (npm pin)": npm_pin,
    }

    unique = set(sources.values())
    if len(unique) == 1:
        print(f"OK: all version sources agree on {cargo_ver}")
        return 0

    print("VERSION MISMATCH:", file=sys.stderr)
    for source, ver in sources.items():
        marker = "✓" if ver == cargo_ver else "✗"
        print(f"  {marker} {source}: {ver}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
