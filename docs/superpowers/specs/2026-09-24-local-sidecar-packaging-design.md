# Local builds must not bundle a placeholder MCP sidecar

- **Date:** 2026-09-24
- **Branch:** `fix/local-build-sidecar-packaging` (spec + plan + implementation on this one branch/PR)
- **Status:** Implemented (PR #229) — code on this branch; §7 scratch-run verification pending
- **Risk tier:** Low–medium. No app code changes. The new `beforeBundleCommand` guard runs inside every release build (macOS universal, Linux, Windows), so a wrong guard blocks releases. That is the reason for §7.

## Problem

Locally built bundles ship a broken MCP sidecar. `tauri.conf.json` declares
`bundle.externalBin: ["binaries/curated-thoughts-mcp"]`. The bundler packages
whatever file sits at `src-tauri/binaries/curated-thoughts-mcp-<triple>`. That
file is gitignored (`src-tauri/binaries/.gitignore`: `curated-thoughts-*`), so
a stale placeholder stays in place between builds.

CI is safe. `build.yml` creates a placeholder only so that `tauri-build`'s
`externalBin` existence check passes during the same cargo build that produces
the real sidecar (`--features mcp-server --bin curated-thoughts`). It then
copies the real binary over the placeholder, restores `+x`, and smoke-tests it
(`tools/smoke_test_mcp_sidecar.sh`, Linux only) before `tauri-action` bundles.

Locally nothing replaces the placeholder:

1. **The README tells developers to create the placeholder** (`README.md`
   "Install & Run": `touch src-tauri/binaries/curated-thoughts-mcp-<triple>`),
   then to run a bare `pnpm tauri build`. Following the docs exactly
   produces a bundle whose sidecar is a 0-byte file. (The Ubuntu dev machine
   had a 10-byte variant dated Aug 27. Either one gets packaged.)
2. **Nothing checks what gets bundled.** `build.rs` is bare
   `tauri_build::build()`. It only checks that the file exists, and it must
   keep accepting the placeholder because CI's sidecar build compiles through
   that same check.
3. **`scripts/install-ct.sh` installs any `.deb` it is given.** It never
   inspects the packaged sidecar.
4. Deleting the build-tree copies does not prevent a recurrence. The next
   `pnpm tauri build` copies the placeholder back in.

## Goals

- A bare `pnpm tauri build` / `pnpm tauri bundle` **fails closed** when the
  sidecar it is about to bundle is not a real executable, on every platform
  and target the release matrix builds.
- A single command gives a correct local bundle by running CI's recipe.
- `install-ct.sh` refuses a `.deb` whose sidecar is broken.
- The README no longer leads to a broken bundle.

## Non-goals

- Changing `build.yml` / `ci.yml` sidecar steps. The guard runs in CI and
  catches drift. Changing the release pipeline is a separate risk the user
  declined.
- A `build.rs` check. It cannot tell CI's in-flight placeholder from a stale
  one.
- Building the sidecar inside `beforeBundleCommand`. That would build it
  twice in CI and needs a separate `--target-dir` so the app binary is not
  clobbered. Rejected in brainstorming as Approach C.
- Local universal/cross-target builds from the wrapper (§3 scope).
- **Known limitation (accepted, Opus m3):** a stale but *real* sidecar passes
  the guard. After the wrapper has run once, a later bare `pnpm tauri build`
  bundles the previous sidecar — structurally sound, possibly old. The guard
  is a placeholder detector, not a freshness check.

## Design

### 1. Shared verifier — `scripts/verify-sidecar.mjs`

A Node ESM module, not a shell script. `beforeBundleCommand` runs through the
platform shell, which is `cmd` on the Windows runner, so a `.sh` hook cannot
run there. Node is always present because the build is driven by pnpm, and
`scripts/` already holds `.mjs` tooling (`check-release-config.mjs`,
`engine-setup-probe.mjs`).

**Exported pure functions** (the unit-test surface, GLM + Opus m2):

```js
// Both return { ok: true } or { ok: false, reason: string }.
export function checkSidecarBinary({ size, mode, head, isWindowsTarget })
export function resolveSidecarPath({ envTriple, hostTriple, repoRoot, binariesDir = 'src-tauri/binaries' })
```

`resolveSidecarPath` owns the triple/.exe/path logic so it is testable without
a filesystem: `envTriple` wins over `hostTriple`; a `windows` triple appends
`.exe`; the result is `<repoRoot>/<binariesDir>/curated-thoughts-mcp-<triple>[.exe]`.

**Library/CLI split (as implemented, supersedes the single-module sketch —
Opus plan-review M1):** `scripts/verify-sidecar-lib.mjs` exports the pure
functions and NEVER calls `process.exit` (vitest imports it; `verifyFile`
returns `{ok, reason, size}`). `scripts/verify-sidecar.mjs` is a thin CLI that
ALWAYS runs the check when executed (no import-vs-script heuristic — a
mismatched symlinked path must not end in a silent exit 0). The spec's earlier
`pathToFileURL` guard idea is superseded by this split.

`checkSidecarBinary` rejects when, given `head` (the first 4 bytes of the file):

- `size < 1_048_576` (1 MiB). Real sidecars are tens of MiB, and both 0- and
  10-byte placeholders fail here.
- the magic header is none of: ELF `7f 45 4c 46`; Mach-O thin
  `fe ed fa ce`/`ce fa ed fe`/`fe ed fa cf`/`cf fa ed fe`; Mach-O fat
  `ca fe ba be`; Mach-O fat64 `ca fe ba bf` (Opus m5: lipo only writes fat64
  on offset overflow today, but the byte costs nothing); PE `4d 5a` (`MZ`).
- `!isWindowsTarget && (mode & 0o111) === 0`. The file is not executable.
  This check is skipped for Windows targets, which have no exec bit.

**CLI entry** (when run as a script) has two modes:

- `node scripts/verify-sidecar.mjs` (the bundle-hook mode) resolves the
  sidecar path as
  `<repo>/src-tauri/binaries/curated-thoughts-mcp-<triple>[.exe]`. Paths are
  resolved from `import.meta.url`, never from the cwd, because Tauri's hook cwd
  is not guaranteed.
  - `<triple>` = `process.env.TAURI_ENV_TARGET_TRIPLE`. This is the triple the
    bundler is bundling for, **not** the host triple. The macOS release builds
    `--target universal-apple-darwin` and bundles
    `curated-thoughts-mcp-universal-apple-darwin`, while CI `touch`es the
    per-arch files as empty placeholders. A host-triple guard would fail every
    macOS release. When the variable is unset (manual invocation), fall back to
    `rustc -vV`'s `host:` line.
  - The `.exe` suffix applies when the triple contains `windows`.
- `node scripts/verify-sidecar.mjs <path>` checks an explicit file and treats
  it as non-Windows unless the path ends in `.exe`. `install-ct.sh` uses this
  mode.

On failure it exits 1 with a message that names the file, its size and the
failed check, and it prints the fix: run `scripts/build-local-bundle.sh`
(local) or check `build.yml`'s "Build MCP sidecar" step (CI).

### 2. Bundle hook — `tauri.conf.json`

```json
"build": {
  "beforeBundleCommand": "node scripts/verify-sidecar.mjs",
  ...
}
```

The path is written relative to the repo root, which is Tauri's hook cwd:
confirmed empirically on CLI 2.11.4 (2026-09-24, GLM spec review — a temporary
hook printed its cwd and `TAURI_ENV_TARGET_TRIPLE`; cwd = repo root, triple =
host). Windows cwd risk is therefore retired to `node` resolution on `cmd`,
which the scratch-workflow run in §7 exercises anyway. If it ever differs, the
object form `{ "script": ..., "cwd": "..." }` is the fallback (Opus m1).

The hook runs on `tauri build` (with bundling) and `tauri bundle`. It does not
run on `tauri dev` or `tauri build --no-bundle`, so dev and test workflows
keep working with the placeholder.

### 3. Local wrapper — `scripts/build-local-bundle.sh`

CI's Linux recipe for the **host triple**. It supports Linux and macOS
non-universal builds. It does not support Windows (no bash) or universal
builds, and it errors out early on those.

```
set -euo pipefail
REPO_ROOT = dirname of script/..        # absolute, cwd-independent
TRIPLE    = rustc -vV | host:
DEST      = src-tauri/binaries/curated-thoughts-mcp-$TRIPLE
BUNDLES   = ${1:-deb on Linux, app on macOS}
1. mkdir -p src-tauri/binaries; [ -e "$DEST" ] || touch "$DEST"    # never truncate a real binary
2. cargo build --release --manifest-path src-tauri/Cargo.toml --features mcp-server --bin curated-thoughts
3. TARGET_DIR=$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)
   # Tauri v2 builds into the workspace-root target/, not src-tauri/target/
4. cp "$TARGET_DIR/release/curated-thoughts" "$DEST"; chmod +x "$DEST"
5. node scripts/verify-sidecar.mjs "$DEST"
6. tools/smoke_test_mcp_sidecar.sh "$DEST"
7. pnpm tauri build --bundles "$BUNDLES"
```

**The order of steps 2 and 7 is load-bearing.** Step 2 overwrites
`$TARGET_DIR/release/curated-thoughts` with an `mcp-server` build. Step 7
rebuilds it without that feature, because the feature set changed, and that
rebuilt binary is the app that gets bundled. This is the same order CI uses.
Never reorder the steps, and never skip step 7's rebuild.

The script checks up front that `jq`, `python3` (needed by the smoke test) and
`node` are installed. It exits on the first failure and never reaches step 7
with an unverified sidecar.

### 4. Installer check — `scripts/install-ct.sh`

After the `.deb` is chosen and before `dpkg -i`:

```
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
dpkg-deb -x "$DEB" "$TMP"
SIDECAR=$TMP/usr/bin/curated-thoughts-mcp        # confirm the packaged path during implementation
[ -e "$SIDECAR" ] || die "no sidecar in $DEB"
node "$REPO_ROOT/scripts/verify-sidecar.mjs" "$SIDECAR" || die "refusing to install: broken sidecar"
```

The installer reuses the verifier and does not re-implement it, so the rules
cannot drift apart. The existing absolute `REPO_ROOT` (commit `fadfbb1`) stays
unchanged. Its "no .deb found" error (line 17) is updated in the same change to
point at `scripts/build-local-bundle.sh` instead of `pnpm tauri build --bundles
deb` (Opus m4 — the current message recommends the exact command that produces
broken bundles).

### 5. README

In "Install & Run":

- Keep the placeholder `touch` block, but say it is **only for `pnpm tauri dev`
  and `cargo test`/clippy**, and that a bundle built on top of it is rejected
  by the bundle guard.
- Replace the bare `pnpm tauri build` with `scripts/build-local-bundle.sh
  [bundles]`, with a one-line explanation of why (the sidecar must be the real
  `mcp-server` build). State that macOS **universal** bundles are CI-only
  (the wrapper refuses them); Apple Silicon developers build for their host
  triple (GLM minor).

## 6. Testing

**Automated** (vitest, runs in `ci.yml` via `pnpm test`):
`src/__tests__/verify-sidecar.test.ts` (or wherever vitest's include glob
picks it up) runs `checkSidecarBinary` against these cases:

| Case | Expect |
|---|---|
| 0 bytes | reject (size) |
| 10 bytes | reject (size) |
| 2 MiB, zero header | reject (magic) |
| 2 MiB, ELF, mode 0644 | reject (exec) |
| 2 MiB, ELF, mode 0755 | ok |
| 2 MiB, Mach-O fat `cafebabe`, 0755 | ok |
| 2 MiB, Mach-O 64 `cffaedfe`, 0755 | ok |
| 2 MiB, `MZ`, mode 0644, Windows target | ok (exec skipped) |
| 2 MiB, `MZ`, mode 0644, non-Windows | reject (exec) |
| 2 MiB, Mach-O fat64 `cafebabf`, 0755 | ok |
| path = a directory | `resolveSidecarPath` ok; stat/read fails → clean reject, not an uncaught EISDIR (GLM) |
| path = symlink → placeholder | reject via `stat` (follows the link; size check catches it) (GLM) |
| path = symlink → real binary | ok via `stat` (GLM) |

It also covers `resolveSidecarPath`: `TAURI_ENV_TARGET_TRIPLE` wins over the
host, and a `windows` triple adds `.exe`.

**Manual, local (Linux):**

1. With the placeholder in place, `pnpm tauri build --bundles deb` fails at the
   bundle hook with the guard's message.
2. `scripts/build-local-bundle.sh` builds, verifies, smoke-tests and bundles.
   Then `dpkg-deb -x` the result and run `/usr/bin/curated-thoughts-mcp --mcp`
   through the smoke test.
3. `install-ct.sh` on a `.deb` rebuilt with the placeholder aborts before
   `dpkg -i`.
4. On macOS, `pnpm tauri build` with the placeholder fails at the hook.

## 7. Testing the guard in release CI before merge

`build.yml` runs only on `v*` tags and `workflow_dispatch`. `ci.yml` never
bundles. **A PR does not exercise the guard.** Without a pre-merge run, the
first macOS-universal and Windows runs of the hook would be the next real
release. Three things are unverified:

- that `TAURI_ENV_TARGET_TRIPLE` equals `universal-apple-darwin` (not an
  arch triple) for `--target universal-apple-darwin`. The variable exists in
  CLI 2.11.4, but its value for universal builds is unconfirmed;
- that the hook's cwd and `node` resolution work under `tauri-action` on
  Windows;
- (Opus M1) that the macOS universal sidecar carries the exec bit — lipo
  writes onto a 0644 `touch`ed placeholder with no `chmod` in `build.yml`, and
  no one has ever observed the resulting mode. If it is 0644, the guard's
  exec-bit check fails every macOS release.

### Resolution: option (c) — throwaway dispatch-only workflow on a scratch branch

Evidence collected 2026-09-24 (Tessera, source-level against tauri-action
`1deb371`):

- `src/index.ts` calls `await buildProject()` **first**; the release is only
  created/uploaded after artifacts exist. A guard failure throws inside the
  build, before any GitHub API call to releases.
- Option **(a) is rejected (Opus M2)**: dispatching `build.yml` with
  `tagName: v__VERSION__` from a branch *succeeding* would
  `getOrCreateRelease('v2.16.1', ...)` — find the live release and upload this
  branch's assets over the published downloads (`releaseDraft: false`, no
  deletion of drafts involved).
- Therefore: a **scratch branch, never merged**, adds a temporary
  `workflow_dispatch`-only workflow (or strips `tagName`/`releaseName`/
  `releaseBody` from the `tauri-action` step — with no release target the
  action builds and uploads only workflow artifacts). Dispatch it on the
  scratch branch; all three release runners (macOS universal, Linux, Windows)
  run the guarded `tauri build` for real. The scratch branch is deleted after.
  This does not violate the "don't change `build.yml`" non-goal because
  nothing merges.

Additional direct evidence already recorded: the published
`Curated.Thoughts_2.16.1_universal.app.tar.gz` (downloaded 2026-09-24) contains
`Contents/MacOS/curated-thoughts-mcp` as `-rwxr-xr-x` (0755) — today's release
sidecar is executable, so the exec-bit check is safe for macOS as-shipped.
(Opus M1's stat-check in the scratch run remains worthwhile as belt-and-braces.)

**The guard must not merge until the scratch-branch run has passed on macOS
universal and Windows, and the result is recorded here:**

> - [ ] Scratch-branch dispatch: macOS universal — hook ran, triple resolved,
>      exec bit verified: (result)
> - [ ] Scratch-branch dispatch: Windows — hook ran, `node` resolved under
>      `cmd`: (result)

## Rollback

Delete `build.beforeBundleCommand` from `tauri.conf.json`. The wrapper, the
installer check and the README change are independent and can stay.
