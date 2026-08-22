# Dependency & Supply-Chain Remediation Design Spec

**Date:** 2026-08-22
**Branch:** `worktree-security-dependabot-remediation` (worktree `.claude/worktrees/security-dependabot-remediation`, based on main @ `fd918cc`)
**Status:** Draft — research sweep of 2026-08-22 (main clean at time of sweep)
**Anchored by:** passive security sweep 2026-08-22: 31 open Dependabot alerts (24 npm, 7 Rust across two manifests), secret scanning enabled with 0 alerts, code scanning never run.

---

## Goal

Close all fixable Dependabot alerts and close the two automation gaps that let them pile up: no scheduled dependency updates (`.github/dependabot.yml` did not exist) and no code scanning (no CodeQL workflow ever ran). One PR, staged commits, so each stage is reviewable and revertable independently.

## Decisions locked (from the 2026-08-22 session)

1. **Close stale Dependabot PRs #30 and #33 in favor of ours** — they are ~5 weeks behind main and would conflict with our lockfile changes. Comment on each pointing at our PR when closing.
2. **One PR with staged commits** — not one PR per stage.
3. **SHA-pin all third-party actions** — first-party `actions/*` are pinned too, for uniformity; Dependabot's new `github-actions` ecosystem keeps every pin current afterward (it understands `ref@sha # comment` pins).

## Current state (verified on stock main @ `fd918cc`, nothing changed yet)

### Baselines (green)

- `src-tauri` cargo tests: green locally, including the MCP-integration feature build (`--features test-utils,mcp-server`).
- Frontend: green — typecheck, lint, vite build, vitest **337 passed / 1 skipped**. First run showed 2 failures while two baselines ran concurrently; clean pass on rerun → flaky-under-load, not real.
- `tools/`: **test compile is broken on stock main** — pre-existing E0061 at `tools/src/bin/semantic_search_profile.rs:55` (`insert_chunk` call passes 5 args; signature gained a trailing `content_hash: &str`, see `src-tauri/src/db/queries.rs:55`). CI never builds tools tests (`ci.yml` only tests `src-tauri`), which is how it rotted. Fixed here because Stage 1 already touches `tools/Cargo.lock`.

### Dependency drift table (verified against both lockfiles)

| Crate | src-tauri | tools | Vulnerability | Fix |
|---|---|---|---|---|
| `sqlx` | 0.8.0 | 0.8.0 | binary-protocol misinterpretation (≤0.8.0) | ≥0.8.1; direct dep of src-tauri |
| `serde_with` | 3.19.0 | 3.20.0 | panic on empty KeyValueMap seq/map (<3.21.0); via tauri-utils | ≥3.21.0 both dirs (= supersedes PR #33) |
| `cmov` | 0.5.3 | 0.5.4 | **wrong results on aarch64 when high register bits set** (<0.5.4); via ctutils | ≥0.5.4 in src-tauri (= supersedes PR #30) |
| `tar` | 0.4.46 | 0.4.45 | PAX header desync (≤0.4.45); via ort-sys/fastembed, dev-only | ≥0.4.46 in tools only |
| `glib` | 0.18.5 | 0.18.5 | unsound `VariantStrIter` (≥0.15,<0.20); via gtk/wry ← tauri | **upstream-blocked** — see below |

The two-lock drift is real and bidirectional (`cmov` fixed in tools but not src-tauri; `tar` fixed in src-tauri but not tools): `tools/` resolves its own lockfile independently. GitHub's scanner also lags — glib 0.18.5 is vulnerable in **both** locks but alert fires on tools/ only.

### glib disposition (upstream-blocked)

`glib ^0.18` comes from the gtk-rs 0.18 stack pulled by `webkit2gtk` ← `wry 0.55.1` ← `tauri 2.x`. Semver cannot resolve `^0.18` to 0.20, so no lockfile command fixes it. Stage 1 attempts `cargo update -p tauri -p wry` within existing requirements first; if gtk stays on 0.18 (expected while tauri 2.x targets webkit2gtk-4.1/gtk 0.18), record accepted risk here and track upstream (tauri/wry bump that moves to gtk 0.20+) rather than force anything.

**Outcome (2026-08-22, executed):** `cargo update -p tauri -p wry` was a no-op (`Locking 0 packages`) — tauri 2.11.1 / wry 0.55.1 / glib 0.18.5 / gtk 0.18.2 are already the latest semver-compatible set, so glib stays 0.18.5. Accepted risk stands; revisit when tauri ships a wry/gtk 0.20+ bump.

### npm alerts

All transitive, all dev/CI tooling — nothing ships in the app bundle. Exposure is pipeline integrity (e.g. `undici` CVE sits in the semantic-release publish path). Fixes are pnpm overrides in `package.json` + regenerated `pnpm-lock.yaml`:

| Package | Resolved | Fix floor | Via |
|---|---|---|---|
| `undici@6` | 6.25.0 | ≥6.28.0 | @actions/http-client@4.0.1 — **per-major override**, forcing 7 breaks it |
| `undici@7` | 7.25.0 | ≥7.29.0 | @semantic-release/github, jsdom |
| `js-yaml` | 4.1.1 | ≥4.3.1 | eslint/@eslint/eslintrc, cosmiconfig (3 advisories, 2 high quadratic-CPU) |
| `postcss` | 8.5.15 | ≥8.5.23 | vite (high: path traversal via sourceMappingURL) |
| `brace-expansion@1` | 1.1.14 | ≥1.1.16 | minimatch@3.1.5 |
| `brace-expansion@5` | 5.0.6 | ≥5.0.7 | @10.2.5 (advisory says "high"; ReDoS-style expansion DoS) |

### Workflow hardening gaps

All three workflows + the local composite action pin actions by mutable tag/ref, not SHA; `build.yml` runs `contents: write`; `release.yml` runs semantic-release with broad permissions. Current refs and their pinned SHAs (fetched 2026-08-22):

| Action | Old ref | Pinned SHA |
|---|---|---|
| `actions/checkout` | `v4` | `11d5960a326750d5838078e36cf38b85af677262` |
| `actions/setup-node` | `v4` | `49933ea5288caeca8642d1e84afbd3f7d6820020` |
| `pnpm/action-setup` | `v4` | `f40ffcd9367d9f12939873eb1018b921a783ffaa` |
| `dtolnay/rust-toolchain` | `stable` | `4360b52568e2003a75bf9bc1d59f33a8e3fc893c` |
| `swatinem/rust-cache` | `v2` | `49a0bdc70d2e1b713ca9e2869b211fcce03d3c1c` |
| `tauri-apps/tauri-action` | `v0` | `fce9c6108b31ea247710505d3aaaa893ee6768d4` |
| `github/codeql-action` (new workflow) | `v3` | `6d786de4d6f3531a740e445b53a42b622bbbace8` |

Pin format keeps the version readable and Dependabot-updatable:
`uses: swatinem/rust-cache@49a0bdc70d2e1b713ca9e2869b211fcce03d3c1c # v2`

Files touched by pinning: `.github/workflows/ci.yml`, `build.yml`, `release.yml`, and `.github/actions/setup-node-pnpm/action.yml` (the composite references `actions/setup-node@v4` + `pnpm/action-setup@v4` itself).

---

## Scope (in)

1. **Cargo updates in BOTH locks** — sqlx, serde_with (both dirs), cmov (src-tauri), tar (tools); glib attempted-then-documented per above.
2. **tools bench fix** — the one-line E0061 repair, so `cargo check --tests` compiles again in tools/.
3. **pnpm overrides** + regenerated `pnpm-lock.yaml`.
4. **`.github/dependabot.yml`** — npm `/`, cargo `/src-tauri`, cargo `/tools`, github-actions `/`; weekly; minor+patch grouped per ecosystem.
5. **CodeQL workflow** — javascript-typescript + rust, `security-extended`, weekly + on push to main + on PRs.
6. **SHA-pin all third-party actions** (table above), including inside the local composite action.
7. **Spec doc** (this file) committed in the same PR.

## Scope (out)

- **glib 0.20 migration** — upstream-blocked; revisit when tauri ships a wry/gtk bump. Accepted risk documented above.
- **CI dual-lock drift assertion** (CI check that both Cargo.locks move together) — noted as convention going forward; dependabot.yml covering both dirs reduces drift structurally. Not built now (YAGNI).
- **Permission tightening** in existing workflows beyond pinning (e.g. dropping `actions: write` from ci.yml rust jobs) — separate hygiene PR if wanted; don't churn CI semantics under a security-fix PR.
- **Any runtime code changes** — except the tools bench one-liner, which un-breaks compiling the crate whose lockfile we touch.

---

## Stage 1 — dependency updates

### Cargo (both locks)

```bash
cargo update --manifest-path src-tauri/Cargo.toml -p sqlx -p serde_with -p cmov
cargo update --manifest-path tools/Cargo.toml -p tar -p serde_with
# glib attempt (expected: no-op or still-0.18 outcome):
cargo update --manifest-path src-tauri/Cargo.toml -p tauri -p wry || true
```

Postconditions (grep both locks):
- `sqlx >= 0.8.1` in **both**; `serde_with >= 3.21.0` in **both**; `cmov == 0.5.4` in src-tauri; `tar >= 0.4.46` in tools.
- Only these packages (+ their required transitive bumps) moved; no mass re-resolution. If `cargo update` wants to move unrelated majors, fall back to `--precise` per package.
- Record final glib/tauri/wry versions in this spec's drift-table section (status edit) once known.

### tools bench fix (`tools/src/bin/semantic_search_profile.rs:55`)

```rust
let cid = insert_chunk(conn, doc_id, &proto, i, "tier_working", &format!("bench-hash-{i}"))
    .expect("chunk");
```

Hash varies per iteration because `idx_chunks_doc_hash (doc_id, content_hash)` is UNIQUE (`src-tauri/src/db/schema.rs:139`) — a constant hash dies on insert #2.

Verify: `cargo check --manifest-path tools/Cargo.toml --tests` → compiles clean (fails on stock main).

### pnpm overrides (`package.json`)

```json
"pnpm": {
  "overrides": {
    "undici@6": ">=6.28.0 <7",
    "undici@7": ">=7.29.0 <8",
    "js-yaml": ">=4.3.1 <5",
    "postcss": ">=8.5.23 <9",
    "brace-expansion@1": ">=1.1.16 <2",
    "brace-expansion@5": ">=5.0.7 <6"
  }
}
```

Ceilings are required: under pnpm 10.33.2 an override value is resolved as a standalone specifier with no relation to the dependent's original range, so a bare `>=floor` drags packages across majors (observed: http-client → undici 7.25.0 out-of-slot, js-yaml → 5.x, the v1 brace-expansion slot dissolved). The cap keeps each floor inside its per-major slot.

Then regenerate and prove the lock took them: `pnpm install` (not frozen), then per-package `pnpm why` shows floors met in-slot (combined multi-name invocation emits nothing on pnpm 10.33.2); `pnpm install --frozen-lockfile` succeeds; full frontend suite green.

## Stage 2 — automation & supply-chain hardening

### `.github/dependabot.yml` (new)

```yaml
version: 2
updates:
  - package-ecosystem: "npm" # pnpm-lock.yaml handled by the npm ecosystem
    directory: "/"
    schedule:
      interval: "weekly"
    groups:
      minor-and-patch:
        update-types:
          - "minor"
          - "patch"

  - package-ecosystem: "cargo"
    directory: "/src-tauri"
    schedule:
      interval: "weekly"
    groups:
      minor-and-patch:
        update-types:
          - "minor"
          - "patch"

  - package-ecosystem: "cargo"
    directory: "/tools"
    schedule:
      interval: "weekly"
    groups:
      minor-and-patch:
        update-types:
          - "minor"
          - "patch"

  - package-ecosystem: "github-actions"
    directory: "/"
    schedule:
      interval: "weekly"
    groups:
      minor-and-patch:
        update-types:
          - "minor"
          - "patch"
```

Grouping applies to scheduled version updates; security updates keep arriving individually regardless.

### `.github/workflows/codeql.yml` (new)

Rust analysis needs a real build, and building src-tauri needs the WebKit system deps — same apt dance as ci.yml (mirror-strip + timeout, per the CI apt-mirror triple-defense; do not simplify any layer out):

```yaml
name: CodeQL

on:
  push:
    branches: [main]
  pull_request:
  schedule:
    - cron: "37 6 * * 1"

permissions:
  contents: read

jobs:
  analyze:
    name: Analyze (${{ matrix.language }})
    runs-on: ubuntu-latest
    permissions:
      security-events: write
      contents: read
    strategy:
      fail-fast: false
      matrix:
        language: [javascript-typescript, rust]
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4

      - name: Install Linux dependencies (Tauri / WebKit)
        if: matrix.language == 'rust'
        run: |
          sudo find /etc/apt -type f \
            \( -name '*.list' -o -name '*.sources' -o -name 'apt-mirrors.txt' \) \
            -exec sed -i 's|azure\.archive\.ubuntu\.com|archive.ubuntu.com|g' {} +
          sudo timeout 5m apt-get \
            -o Acquire::http::Timeout=120 \
            -o Acquire::https::Timeout=120 \
            -o Acquire::Retries=0 \
            update
          sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf pkg-config libssl-dev

      - uses: github/codeql-action/init@6d786de4d6f3531a740e445b53a42b622bbbace8 # v3
        with:
          languages: ${{ matrix.language }}
          queries: security-extended

      - if: matrix.language == 'rust'
        uses: github/codeql-action/autobuild@6d786de4d6f3531a740e445b53a42b622bbbace8 # v3

      - uses: github/codeql-action/analyze@6d786de4d6f3531a740e445b53a42b622bbbace8 # v3
        with:
          category: "/language:${{ matrix.language }}"
```

Notes: cron Monday 06:37 UTC (off the :00/:30 pileup). `security-events: write` is job-scoped, not workflow-wide. First rust run builds the full workspace cold — slow but bounded by autobuild; acceptable for weekly cadence + PR runs.

### SHA pins applied everywhere

Replace every `uses:` per the table (including `# vX` comments), in all three workflows + the composite action. No behavioral params change.

---

## Verification plan (whole PR)

1. `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server` → green, same as baseline.
2. `cargo check --manifest-path tools/Cargo.toml --tests` → compiles (new win vs. broken baseline).
3. `pnpm install --frozen-lockfile && pnpm run build && pnpm lint && pnpm test` → green, 337 passed / 1 skipped expected.
4. `git grep -n 'uses:' .github` shows no bare-tag third-party refs remaining.
5. Push → CI green on the PR; Dependabot picks up new config without erroring (check the Insights → Dependencies tab after merge).
6. Close #30/#33 with comment superseded-by-our-PR.

## Risks & mitigations

- **Override too aggressive for @actions/http-client** → mitigated by per-major undici selectors; verify `@actions/http-client` still resolves to undici 6.x via `pnpm why`.
- **CodeQL rust build fails on runner** → the apt block mirrors proven ci.yml incantation; if autobuild chokes on workspace features, scope init with `paths`/`path-filters` in a follow-up rather than weakening security-extended.
- **cargo update pulls surprise majors** → use `--precise` fallback; diff lockfiles before committing.
- **Pinned dtolnay `stable` freezes toolchain freshness** → intended; Dependabot PRs move the pin when the branch advances.

## Relationship to prior specs/PRs

- Supersedes open PRs **#30** (`cmov 0.5.4` in src-tauri) and **#33** (`serde_with 3.21.0` in src-tauri): both fixes land here across both locks; those branches get closed with a pointer.
- No prior design spec covers dependency posture; this becomes the canonical reference for the two-Cargo.lock rule ("dependency changes touch BOTH locks").
