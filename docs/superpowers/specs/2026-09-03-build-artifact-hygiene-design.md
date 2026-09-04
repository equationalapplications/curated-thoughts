# Spec: build-artifact hygiene via Cargo workspace unification

**Repo:** curated-thoughts · **Type:** build-system restructure · **Priority:** P1
**Status:** Draft
**Branch:** `spec/build-artifact-hygiene`
**Context:** A 2026-09-03 disk-exhaustion incident (data volume at 100%, 2.2 GiB free) traced to duplicated Cargo build output.

## 1. Problem

`src-tauri/` and `tools/` are two independent Cargo projects, not a workspace. There is no `.cargo/config.toml` anywhere in the repo and `CARGO_TARGET_DIR` is unset. Each project therefore compiles the same dependency graph into its own private `target/`, and each git worktree multiplies that again.

Measured on 2026-09-03:

| Path | Size | Disposition |
|------|-----:|-------------|
| `src-tauri/target/` | 107 GB | deleted during the incident |
| `tools/target/` | 26 GB | **still on disk** |
| `.worktrees/*/src-tauri/target/` | 302 MB | still on disk |
| `~/.cargo/registry` | 1.6 GB | shared, healthy — not a target |

The duplication is near-total, not incidental: the two lockfiles are 9,279 and 9,210 lines, and `tools` already path-depends on `src-tauri` (`tauri_app_lib = { path = "../src-tauri", package = "curated-thoughts" }`). The two projects compile substantially the same graph twice.

A second, related defect: because there is no workspace, shared dependency versions are kept in sync **by hand-written comment**. `tools/Cargo.toml` states the `notify`/`sha2` pins "match the pins in `src-tauri/Cargo.toml` to keep the workspace on a single notification backend and a single SHA-2 implementation." The codebase already reasons about itself as a workspace; it simply is not one. Comment-enforced version parity is a silent-drift footgun.

## 2. Approach

Promote the two projects into a real Cargo workspace (root virtual manifest), and lift the hand-pinned shared dependencies into `[workspace.dependencies]`.

Rejected alternatives:

- **Committed `.cargo/config.toml` with `build.target-dir`.** Smaller diff and no lockfile merge, but it relocates the symptom rather than removing the cause: two lockfiles keep resolving independently, so the shared directory still stores divergent variants, and the comment-enforced pinning survives.
- **Cross-worktree shared target dir.** Requires an absolute path, so it cannot be committed (a checked-in `.cargo/config.toml` resolves relative to its own file, giving each worktree its own target again). It also serializes builds, because Cargo takes an exclusive lock on the target directory. Deliberately out of scope — see §7.

## 3. Workspace structure

A new root `Cargo.toml` virtual manifest:

```toml
[workspace]
members = ["src-tauri", "tools"]
resolver = "2"
```

**`resolver = "2"` is mandatory and is the highest-risk single line in this change.** A virtual workspace manifest defaults to resolver **v1** even when every member declares `edition = "2021"`. Omitting it silently changes feature unification across the entire dependency graph of a Tauri application. It must be present in the first commit that creates the manifest, not added later.

Confirmed absent from both member manifests, so nothing needs relocating to the root: no `[profile.*]` sections (which are ignored with a warning in non-root members), no `[patch]`, no `[replace]`, no existing `resolver` key.

Outcomes:

- `src-tauri/target/` and `tools/target/` collapse into a single root `target/`.
- `src-tauri/Cargo.lock` and `tools/Cargo.lock` are replaced by one root `Cargo.lock`.
- `cargo clean` reclaims the 26 GB currently held by `tools/target/`.

**The two removed lockfiles are load-bearing in the release pipeline, not just in builds.** `scripts/update-versions.cjs` (the semantic-release `prepareCmd`) ran `cargo metadata` in `src-tauri/` and `tools/` in turn and read back each member `Cargo.lock` to verify the version bump; after unification both reads are `ENOENT` and every release fails at prepare. `.releaserc.json` likewise lists `src-tauri/Cargo.lock` and `tools/Cargo.lock` as `@semantic-release/git` assets. Both are repointed to the single root `Cargo.lock`, and the script's second `cargo metadata` pass drops entirely — one workspace resolve now keeps the `tools` path dependency in sync by construction. This is the same silent-until-release failure class as the `rust-cache` `workspaces:` entries below, and a literal search for `Cargo.lock` is what finds it.

## 4. Shared pins → `[workspace.dependencies]`

All fifteen dependencies currently duplicated across both members move to `[workspace.dependencies]`, with members referencing them as `dep.workspace = true`: `serde`, `serde_json`, `anyhow`, `tokio`, `rusqlite`, `notify`, `sha2`, `dirs`, `flate2`, `fs4`, `walkdir`, `rmcp`, `schemars`, `tempfile`, `temp-env`.

The objective is to eliminate manual version synchronization across every shared dependency, so the set is defined by what is actually duplicated, not by what was most visible.

**Member feature sets differ and must not be flattened.** Known divergences:

| Dependency | `src-tauri` | `tools` |
|------------|-------------|---------|
| `tokio` | `["full", "test-util"]` | `["rt", "macros", "signal"]` |
| `rusqlite` | `["bundled", "backup"]` | `["bundled"]` |
| `rmcp` | `["macros", "transport-io", "schemars"]`, `optional`; **and** `["client", "transport-child-process"]` as a dev-dependency | `["macros", "transport-io", "schemars"]` |
| `schemars` | `["derive"]`, `optional` | `["derive"]` |

Two rules govern how each pin is written:

1. **Where every site shares one feature set,** the workspace entry carries the version and those features, and members inherit with a bare `dep.workspace = true`.
2. **Where feature sets diverge across sites,** the workspace entry pins the **version only**, and each site declares its own features alongside `workspace = true`. This applies to `tokio`, `rusqlite`, and `rmcp`.

`rmcp` is the case that forces rule 2. It appears at three sites: an optional dependency in src-tauri gated behind the `mcp-server` feature, a dev-dependency in src-tauri with a completely different feature set, and a required dependency in tools. If the workspace entry carried features, src-tauri's dev-dependency would silently acquire `macros`, `transport-io`, and `schemars` in test builds that do not enable `mcp-server`. A version-only pin preserves all three sites exactly.

`optional = true` is a member-level attribute and is preserved by writing `{ workspace = true, optional = true }`; it does not move to the workspace entry. This matters for `rmcp` and `schemars`, both of which `mcp-server` activates via `dep:`.

Collapsing feature sets into a single union would silently widen the shipped application's feature surface. Any dependency whose feature sets cannot be expressed by these two rules stays a per-member dependency — deduplication is not worth a behavioral change.

`tools`-only and `src-tauri`-only dependencies stay in their member manifests. This section is about the shared set, not about centralizing everything.

## 5. CI and release paths

`.github/workflows/build.yml` hardcodes the target directory in four places — lines 88, 92, 132, and 133 — to stage the sidecar binary and the macOS universal binary. These break the moment the target directory moves.

Retargeting them to `target/...` would relocate the same landmine. Instead, derive the path:

```sh
cargo metadata --format-version 1 | jq -r .target_directory
```

This makes the release pipeline immune to any future target-directory change, and is the guardrail that prevents recurrence of this class of breakage.

**A literal search for `src-tauri/target` is not sufficient to find every reference.** Three `Swatinem/rust-cache` steps — one in `build.yml`, two in `ci.yml` — configure the cache with an arrow syntax, `workspaces: ./src-tauri -> target`, that such a search does not match. These must be repointed to `. -> target`. They are the more dangerous class of stale reference precisely because they do not fail: a wrong `workspaces` value silently misses the cache and turns every CI run in both workflows into a cold Tauri compile, which reads as general CI slowness rather than as a regression from this change.

`cargo metadata` cannot help here — the action needs a literal path in YAML, not a shell-derived one — so this reference remains hardcoded and must be updated by hand if the layout changes again.

**This is release-only code.** PR CI does not exercise the sidecar staging or the universal-binary lipo step, so a regression here surfaces at release time, not in review. It therefore requires the explicit `--release` verification in §6.

**Do not repoint the sidecar build at `tools/`.** `tools/Cargo.toml` declares a `[[bin]] name = "curated-thoughts-mcp"`, which makes it look like the natural source of the sidecar. It is not the one CI ships. `build.yml` builds src-tauri's `curated-thoughts` binary with `--features mcp-server` and copies that artifact into `src-tauri/binaries/` under the sidecar's name. The placeholder `touch` immediately above the build exists only to satisfy `tauri-build`'s `externalBin` check during that same cargo invocation. Changing which crate produces the sidecar is out of scope here and would be a behavioral change, not a path fix.

Existing `--manifest-path src-tauri/Cargo.toml` invocations in `ci.yml` (lines 88, 94, 131) and `build.yml` (lines 84, 127) continue to work unchanged; they resolve into the shared target automatically.

## 6. Verification and rollback

**The two lockfiles were already drifted before this change.** Measured on 2026-09-03, `src-tauri/Cargo.lock` and `tools/Cargo.lock` disagreed on **56 packages** — `tools` resolved `tauri` 2.11.1, `regex` 1.12.3, `uuid` 1.23.1 and `tray-icon` 0.23.1 where the application resolved 2.11.5, 1.13.1, 1.25.0 and 0.24.2. The comment-enforced pinning described in §1 had already failed in practice: the headless CLI was compiling against a different Tauri than the application it links into.

This makes "no version changes" unsatisfiable by construction. A single lockfile holds one version per crate, so unification must choose. The rule is therefore directional:

- **Seed the root lockfile from `src-tauri/Cargo.lock`** (`cp src-tauri/Cargo.lock Cargo.lock`), then let Cargo resolve only what is missing. This holds the shipped application's graph fixed and moves `tools` onto its pins.
- **Never run `cargo generate-lockfile` to create the root lockfile.** It re-resolves the entire graph to latest-compatible and discards existing pins, producing hundreds of unrelated version changes that are trivially mistaken for a merge artifact.

Convergence in the other direction — pinning the application backward to the CLI's older versions — is rejected: it regresses the product to satisfy a development tool.

The lockfile merge is the only step that can change resolved dependency versions, so it gates everything downstream. Ordered:

1. Create the workspace manifest and merge lockfiles locally.
2. Capture `cargo tree` before and after; diff resolved versions and justify every change. Unintended major or minor bumps block the change.
3. Run the exact `ci.yml` test commands, including `--features test-utils,mcp-server -- --test-threads=1`.
4. Run a `--release` build and confirm sidecar staging, because §5 is not covered by PR CI.
5. **Only after 1–4 are green,** modify `build.yml`.

Rollback is a plain `git revert`; both member lockfiles are restored from git history. No data migration and no persisted state are involved, so revert is complete and lossless.

Note for the implementer: a pre-existing test flake, `paths::tests::brain_paths_re_exports_canonical_type`, is a parallel-execution race unrelated to this change. Do not treat it as a regression signal.

## 7. Guardrails and non-goals

Guardrails:

- Root `.gitignore` gains `/target/`.
- The now-dead `tools/target/` entry in the root `.gitignore` and the `/target/` entry in `src-tauri/.gitignore` are removed, so one rule governs build output.
- CI derives the target path (§5) rather than hardcoding it.

Explicit non-goals, deliberately excluded to keep this change revertible and reviewable:

- **Cross-worktree target sharing.** Each worktree still builds its own copy. If worktrees prove to be the dominant consumer after this lands, that is a follow-up spec — it needs an uncommittable absolute path and accepts build serialization.
- **`cargo-sweep` or scheduled pruning.** No automation in this change.
- **Stale branch, worktree, and stash pruning.** The repo currently carries 8 local branches (6 already merged into main), 6 worktrees, and 3 stashes. This is real accumulation, but it is repo hygiene, not build-system structure.

## 8. Success criteria

- One `target/` directory and one `Cargo.lock` at the repo root.
- `tools/target/` and `src-tauri/target/` no longer created by any build.
- The **shipped application's** resolved dependency graph is unchanged: zero version changes in `curated-thoughts`. Additions are permitted only where they are `tools`-only subtrees entering the shared lock.
- `tools` converging onto the application's pins is expected and required, not a violation — see §6.
- `ci.yml` test commands pass, and a `--release` build produces a correctly staged sidecar.
- `build.yml` contains no hardcoded target path.
- All fifteen shared pins are enforced by Cargo, and the "must match the pins in src-tauri" comments in `tools/Cargo.toml` are deleted as obsolete.
- No member's resolved feature set changes; `rmcp`'s three declaration sites keep their distinct features.
