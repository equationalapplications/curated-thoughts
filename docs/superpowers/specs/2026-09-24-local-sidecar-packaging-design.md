# Local builds must not bundle a placeholder MCP sidecar

- **Date:** 2026-09-24
- **Branch:** `fix/local-build-sidecar-packaging` (spec + plan + implementation on this one branch/PR)
- **Status:** Draft — awaiting user review
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

## Design

### 1. Shared verifier — `scripts/verify-sidecar.mjs`

A Node ESM module, not a shell script. `beforeBundleCommand` runs through the
platform shell, which is `cmd` on the Windows runner, so a `.sh` hook cannot
run there. Node is always present because the build is driven by pnpm, and
`scripts/` already holds `.mjs` tooling (`check-release-config.mjs`,
`engine-setup-probe.mjs`).

**Exported pure function** (the unit-test surface):

```js
// Returns { ok: true } or { ok: false, reason: string }.
export function checkSidecarBinary({ size, mode, head, isWindowsTarget })
```

Given `head` (the first 4 bytes of the file), it rejects when:

- `size < 1_048_576` (1 MiB). Real sidecars are tens of MiB, and both 0- and
  10-byte placeholders fail here.
- the magic header is none of: ELF `7f 45 4c 46`; Mach-O thin
  `fe ed fa ce`/`ce fa ed fe`/`fe ed fa cf`/`cf fa ed fe`; Mach-O fat/universal
  `ca fe ba be`; PE `4d 5a` (`MZ`).
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

The path is written relative to the repo root, which is Tauri's hook cwd for
`beforeBuildCommand` (`pnpm run build` resolves the root `package.json`).
Verify this during implementation. If the cwd differs, switch to the object
form `{ "script": ..., "cwd": ".." }`, which CLI 2.11.4 supports.

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
unchanged.

### 5. README

In "Install & Run":

- Keep the placeholder `touch` block, but say it is **only for `pnpm tauri dev`
  and `cargo test`/clippy**, and that a bundle built on top of it is rejected
  by the bundle guard.
- Replace the bare `pnpm tauri build` with `scripts/build-local-bundle.sh
  [bundles]`, with a one-line explanation of why (the sidecar must be the real
  `mcp-server` build).

## Testing

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

It also covers path resolution: `TAURI_ENV_TARGET_TRIPLE` wins over the host,
and a `windows` triple adds `.exe`.

**Manual, local (Linux):**

1. With the placeholder in place, `pnpm tauri build --bundles deb` fails at the
   bundle hook with the guard's message.
2. `scripts/build-local-bundle.sh` builds, verifies, smoke-tests and bundles.
   Then `dpkg-deb -x` the result and run `/usr/bin/curated-thoughts-mcp --mcp`
   through the smoke test.
3. `install-ct.sh` on a `.deb` rebuilt with the placeholder aborts before
   `dpkg -i`.
4. On macOS, `pnpm tauri build` with the placeholder fails at the hook.

## 7. Open question — exercising the guard in release CI before merge

`build.yml` runs only on `v*` tags and `workflow_dispatch`. `ci.yml` never
bundles. **A PR does not exercise the guard.** Without a pre-merge run, the
first macOS-universal and Windows runs of the hook would be the next real
release. Two things are unverified:

- that `TAURI_ENV_TARGET_TRIPLE` equals `universal-apple-darwin` (not an
  arch triple) for `--target universal-apple-darwin`. The variable exists in
  CLI 2.11.4, but its value for universal builds is unconfirmed;
- that the hook's cwd and `node` resolution work under `tauri-action` on
  Windows.

`workflow_dispatch` of `build.yml` on this branch would run the hook for real,
but `tauri-action` is configured with `tagName`/`releaseName` and may create or
modify a GitHub Release. Before implementation, resolve which option applies:

- **(a)** confirm from `tauri-action` v1.0.0 behavior that a dispatch from a
  non-tag ref publishes nothing, or only a draft that can be deleted, then
  dispatch on this branch;
- **(b)** otherwise, verify the universal triple value locally on macOS
  (`pnpm tauri build --target universal-apple-darwin` with a temporary
  `beforeBundleCommand` that echoes the env), accept the Windows cwd risk, and
  watch the next release's Build run closely with the rollback ready (remove
  the one `beforeBundleCommand` line).

The guard must not merge until (a) or (b) is done and its result is recorded
here.

## Rollback

Delete `build.beforeBundleCommand` from `tauri.conf.json`. The wrapper, the
installer check and the README change are independent and can stay.
