# Release Checklist — memvid-core

Use this checklist for every release, from patch to major.  Items marked
**[CI]** are enforced automatically; all others require a human sign-off.

---

## 0. Pre-work

- [ ] Confirm you are on a clean branch based on `main` (or the release branch)
- [ ] Ensure `git status` is clean and no stashed work exists
- [ ] Pull the latest remote to avoid merge conflicts

---

## 1. Code Quality

### 1.1 Formatting **[CI]**

```bash
cargo fmt --all -- --check
```

- [ ] Zero formatting diffs

### 1.2 Linting **[CI]**

```bash
cargo clippy --all-targets --features "lex,pdf_extract,simd" -- -D warnings \
  -A clippy::non_std_lazy_statics
```

- [ ] Zero warnings, zero errors

### 1.3 Test suite **[CI]**

```bash
# Default feature set
cargo test --features "lex,pdf_extract,simd"

# Minimal kernel (no optional features)
cargo test --no-default-features

# MSRV check (1.85.0)
cargo +1.85.0 test
```

- [ ] All three pass

### 1.4 Panic audit

```bash
python3 scripts/audit_panics.py --strict
# or via wrapper: ./scripts/audit_panics.sh --strict
```

- [ ] Exit code 0 (no unallowlisted production panic sites)
- [ ] Review `docs/audits/panic_unwrap_audit.md` for any new findings
- [ ] If new `unwrap` / `expect` sites were introduced, update `tools/panic_allowlist.toml`
  and the audit document

---

## 2. Version Consistency

### 2.1 Bump the version

The canonical version lives in `Cargo.toml`.  Update it first:

```toml
[package]
version = "X.Y.Z"
```

Then verify all secondary locations agree:

```bash
python3 scripts/check_version_consistency.py
```

Secondary locations that must match:
- `Cargo.toml` [`package.version`]
- `Cargo.lock` (regenerated automatically by cargo)
- `docker/cli/Dockerfile` — `LABEL org.opencontainers.image.version`
  *(only the CLI image carries this label; `docker/core/Dockerfile` does not)*
- Any pinned version references in `README.md`

- [ ] `check_version_consistency.py` exits 0 **[CI]**

### 2.2 Update CHANGELOG.md

Format follows [Keep a Changelog](https://keepachangelog.com) + SemVer:

```markdown
## [X.Y.Z] - YYYY-MM-DD
### Added
- …
### Changed
- …
### Fixed
- …
### Security
- …
```

- [ ] `[Unreleased]` section is renamed to `[X.Y.Z] - YYYY-MM-DD`
- [ ] A new empty `[Unreleased]` section is added at the top
- [ ] Breaking changes are called out under `### Changed` or a dedicated
  `### Breaking Changes` subsection for major bumps

---

## 3. Documentation

- [ ] `README.md` — quick-start example compiles against the new version
- [ ] `docs/ARCHITECTURE.md` — updated if any module was added or removed
- [ ] `docs/agent_memory.md` — updated if the memory-layer API changed
- [ ] `docs/audits/panic_unwrap_audit.md` — updated if new panic sites were
  introduced (re-run the audit if in doubt)
- [ ] Public API doc comments (`cargo doc --no-deps`) build without warnings

---

## 4. Proofs

Run the baseline proof to confirm the existing `.mv2` file format is backward-
compatible:

```bash
chmod +x scripts/proof_baseline.sh scripts/proof_clean_clone.sh
./scripts/proof_baseline.sh
./scripts/proof_clean_clone.sh
```

- [ ] `proof_baseline.sh` exits 0 (existing fixture files round-trip correctly)
- [ ] `proof_clean_clone.sh` exits 0 (clean-clone build + smoke test passes)
  > **Note:** `proof_clean_clone.sh` uses `git clone --local` internally and
  > therefore requires a real git checkout (not a zip export or shallow clone).

---

## 5. Security

- [ ] Dependency audit:

  ```bash
  cargo audit
  ```

  Zero high/critical advisories, or documented exceptions.

- [ ] No new `unsafe` blocks introduced without a safety comment
- [ ] Encryption feature (`--features encryption`) compiles and its tests pass:

  ```bash
  cargo test --features "lex,encryption"
  ```

---

## 6. Tag and Publish

### 6.1 Commit the version bump

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md
# Include any documentation/allowlist changes
git add docs/ tools/
git commit -m "chore: release vX.Y.Z"
```

### 6.2 Tag

```bash
git tag -s "vX.Y.Z" -m "memvid-core vX.Y.Z"
git push origin main --tags
```

The signed tag triggers:
- **`ci.yml`** — full test matrix
- **`docker-release.yml`** — Docker image `memvid/cli:X.Y.Z` on Docker Hub

After pushing the tag, create a GitHub Release in the web UI (or via
`gh release create vX.Y.Z`). Creating the Release triggers:
- **`generator-generic-ossf-slsa3-publish.yml`** — SLSA Level 3 provenance
  artifact attached to the GitHub Release
  (also triggerable via `workflow_dispatch` for re-attestation without a
  new release)

### 6.3 Dry-run publish

```bash
cargo publish --dry-run
```

- [ ] No errors, no warnings about missing metadata

### 6.4 Publish

```bash
cargo publish
```

- [ ] Crate visible on crates.io within ~60 s

---

## 7. Post-release

- [ ] Verify Docker image is available:

  ```bash
  docker pull memvid/cli:X.Y.Z
  ```

- [ ] Verify SLSA provenance attestation is attached to the GitHub Release
  (check the Assets section in the release page)
- [ ] Close / update the release milestone in the issue tracker (if applicable)
- [ ] Announce in the project's communication channel (Discussions, Slack, etc.)

---

## 8. Rollback Plan

If a critical bug is discovered within 24 h of a release:

1. `cargo yank --version X.Y.Z` to prevent new installs from crates.io
2. Push a `vX.Y.Z+1` patch release following this checklist
3. `docker manifest` the old tag to the new image once the patch is published

---

## 9. Reference: CI Jobs

| Job | Trigger | Key command |
|-----|---------|-------------|
| `test-default-profile` | push / PR | `cargo test --features "lex,pdf_extract,simd"` |
| `test-minimal-kernel` | push / PR | `cargo test --no-default-features` |
| `test-msrv` | push / PR | `cargo +1.85.0 test` |
| `lint` | push / PR | `cargo clippy --all-targets --features "lex,pdf_extract,simd" -- -D warnings` |
| `version-consistency` | push / PR | `python3 scripts/check_version_consistency.py` |
| `panic-audit` | push / PR | `python3 scripts/audit_panics.py --strict` |
| `docker-release` | tag `v*`, `workflow_dispatch` | Docker build + push to Docker Hub (`memvid/cli`) |
| `ossf-slsa3` | GitHub Release created, `workflow_dispatch` | SLSA Level 3 provenance generation |
