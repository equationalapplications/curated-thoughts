# Local Sidecar Packaging Guard — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Local `pnpm tauri build` fails closed when the MCP sidecar it is about to bundle is a placeholder, and a single wrapper command produces a correct bundle.

**Architecture:** A Node ESM verifier (`scripts/verify-sidecar.mjs`) wired as `build.beforeBundleCommand` in `tauri.conf.json` (fail-closed guard on every bundling build, all platforms). A bash wrapper (`scripts/build-local-bundle.sh`) runs CI's sidecar recipe then bundles. `scripts/install-ct.sh` gains a pre-`dpkg` gate reusing the verifier. README stops recommending the broken bare build.

**Tech Stack:** Node ESM (no new deps — `node:fs/promises`, `node:path`, `node:url`), vitest (existing), bash, tauri CLI 2.11.4 hooks.

**Spec:** `docs/superpowers/specs/2026-09-24-local-sidecar-packaging-design.md` (as amended by review round 1, commit `c9beb9c` — including §7's option (c) resolution)

## Global Constraints

- No new npm dependencies. Node stdlib only in the verifier.
- Verifier is pure-library + guarded CLI: importing it must never `process.exit` (vitest imports it). Script mode only when `import.meta.url === pathToFileURL(process.argv[1]).href`.
- Magic headers accepted: ELF `7f 45 4c 46`; Mach-O thin `fe ed fa ce` / `ce fa ed fe` / `fe ed fa cf` / `cf fa ed fe`; Mach-O fat `ca fe ba be`; Mach-O fat64 `ca fe ba bf`; PE `4d 5a`.
- Size floor: `1_048_576` (1 MiB).
- Exec-bit check: `!isWindowsTarget && (mode & 0o111) === 0` → reject. Skipped for Windows targets.
- `resolveSidecarPath`: `envTriple` wins over `hostTriple`; triple containing `windows` appends `.exe`.
- `tauri.conf.json` hook: `"beforeBundleCommand": "node scripts/verify-sidecar.mjs"` (string form; hook cwd = repo root, empirically confirmed on CLI 2.11.4).
- `build.yml` / `ci.yml` are NOT modified. Never reorder the wrapper's build steps (spec §3: step 2 then step 7, load-bearing).
- All repo commits: plain `feat:`/`fix:`/`docs:`/`test:` conventional commits; repo merges with merge commits (never squash) but that is the merger's concern.
- Node on this machine: run `node --version` once; scripts must not use features newer than the installed runtime.

---

### Task 1: verifier — library + CLI split (TDD)

**Files:**
- Create: `scripts/verify-sidecar-lib.mjs` (pure library — no `process.exit` anywhere)
- Create: `scripts/verify-sidecar.mjs` (thin CLI — imports the lib, ALWAYS runs main)
- Test: `src/__tests__/verify-sidecar.test.ts`

**Interfaces:**
- Consumes: nothing (leaf module).
- Produces (Tasks 2–5 and tests import from `verify-sidecar-lib.mjs`):
  - `checkSidecarBinary({ size, mode, head, isWindowsTarget })` → `{ ok: true } | { ok: false, reason: string }`
  - `resolveSidecarPath({ envTriple, hostTriple, repoRoot, binariesDir = 'src-tauri/binaries' })` → `string` (absolute path, `.exe` appended iff triple contains `windows`)
  - `verifyFile(filePath, { isWindowsTarget })` → `{ ok: true, size } | { ok: false, reason }` — pure-ish: stats/reads, NEVER exits (Opus M3; vitest exercises this directly)
  - CLI `node scripts/verify-sidecar.mjs [path]`: no arg = hook mode (env triple, else `rustc -vV` host); with arg = explicit file, non-Windows unless path ends `.exe`. Exit 0/1; on failure prints file, size/reason, and the fix line: `Fix: run scripts/build-local-bundle.sh (local) or check build.yml's "Build MCP sidecar" step (CI).`

The lib/CLI split replaces the `import.meta.url === pathToFileURL(argv[1])`
guard (Opus M1: symlinked repo paths make that comparison unreliable, and a
false negative means the guard exits 0 on a placeholder — silent pass). With a
separate CLI file there is no mode detection at all: importing the lib never
exits; running the CLI always checks.

- [ ] **Step 1: Write the failing tests**

Create `src/__tests__/verify-sidecar.test.ts`:

```ts
import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { mkdtemp, writeFile, symlink, mkdir, chmod } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import {
  checkSidecarBinary,
  resolveSidecarPath,
  verifyFile,
} from '../../scripts/verify-sidecar-lib.mjs';

const MiB = 1024 * 1024;
const ELF = [0x7f, 0x45, 0x4c, 0x46];
const MZ = [0x4d, 0x5a];
const FAT = [0xca, 0xfe, 0xba, 0xbe];
const FAT64 = [0xca, 0xfe, 0xba, 0xbf];
const MO64 = [0xcf, 0xfa, 0xed, 0xfe];

describe('checkSidecarBinary', () => {
  it('rejects 0 bytes', () => {
    expect(checkSidecarBinary({ size: 0, mode: 0o755, head: ELF, isWindowsTarget: false }).ok).toBe(false);
  });
  it('rejects 10 bytes', () => {
    expect(checkSidecarBinary({ size: 10, mode: 0o755, head: ELF, isWindowsTarget: false }).ok).toBe(false);
  });
  it('rejects 2 MiB with zero header (magic)', () => {
    expect(checkSidecarBinary({ size: 2 * MiB, mode: 0o755, head: [0, 0, 0, 0], isWindowsTarget: false }).ok).toBe(false);
  });
  it('rejects 2 MiB ELF mode 0644 (exec)', () => {
    expect(checkSidecarBinary({ size: 2 * MiB, mode: 0o644, head: ELF, isWindowsTarget: false }).ok).toBe(false);
  });
  it('rejects when mode is missing (fail closed, Opus m6)', () => {
    expect(checkSidecarBinary({ size: 2 * MiB, head: ELF, isWindowsTarget: false }).ok).toBe(false);
  });
  it('accepts 2 MiB ELF mode 0755', () => {
    expect(checkSidecarBinary({ size: 2 * MiB, mode: 0o755, head: ELF, isWindowsTarget: false }).ok).toBe(true);
  });
  it('accepts Mach-O fat, fat64, thin-64 0755', () => {
    for (const head of [FAT, FAT64, MO64]) {
      expect(checkSidecarBinary({ size: 2 * MiB, mode: 0o755, head, isWindowsTarget: false }).ok).toBe(true);
    }
  });
  it('accepts MZ 0644 for Windows target (exec skipped)', () => {
    expect(checkSidecarBinary({ size: 2 * MiB, mode: 0o644, head: MZ, isWindowsTarget: true }).ok).toBe(true);
  });
  it('rejects MZ 0644 for non-Windows target (exec)', () => {
    expect(checkSidecarBinary({ size: 2 * MiB, mode: 0o644, head: MZ, isWindowsTarget: false }).ok).toBe(false);
  });
  it('names the failed check in reason', () => {
    const r = checkSidecarBinary({ size: 10, mode: 0o755, head: ELF, isWindowsTarget: false });
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.reason).toMatch(/size/i);
  });
});

describe('resolveSidecarPath', () => {
  const repo = '/repo';
  it('envTriple wins over hostTriple', () => {
    expect(resolveSidecarPath({ envTriple: 'universal-apple-darwin', hostTriple: 'x86_64-unknown-linux-gnu', repoRoot: repo }))
      .toBe('/repo/src-tauri/binaries/curated-thoughts-mcp-universal-apple-darwin');
  });
  it('falls back to hostTriple when env unset or empty', () => {
    expect(resolveSidecarPath({ envTriple: undefined, hostTriple: 'x86_64-unknown-linux-gnu', repoRoot: repo }))
      .toBe('/repo/src-tauri/binaries/curated-thoughts-mcp-x86_64-unknown-linux-gnu');
    expect(resolveSidecarPath({ envTriple: '', hostTriple: 'aarch64-apple-darwin', repoRoot: repo }))
      .toBe('/repo/src-tauri/binaries/curated-thoughts-mcp-aarch64-apple-darwin');
  });
  it('appends .exe for windows triples', () => {
    expect(resolveSidecarPath({ envTriple: 'x86_64-pc-windows-msvc', hostTriple: undefined, repoRoot: repo }))
      .toBe('/repo/src-tauri/binaries/curated-thoughts-mcp-x86_64-pc-windows-msvc.exe');
  });
});

describe('verifyFile (filesystem cases, spec §6)', () => {
  let dir: string;
  beforeAll(async () => { dir = await mkdtemp(path.join(tmpdir(), 'sidecar-test-')); });
  afterAll(async () => { await (await import('node:fs/promises')).rm(dir, { recursive: true, force: true }); });

  it('rejects a directory path (EISDIR → clean reject, not a crash)', async () => {
    const sub = path.join(dir, 'a-dir');
    await mkdir(sub);
    const r = await verifyFile(sub, { isWindowsTarget: false });
    expect(r.ok).toBe(false);
  });
  it('rejects a symlink to a placeholder (stat follows the link)', async () => {
    const stub = path.join(dir, 'stub');
    await writeFile(stub, '#!/bin/sh\n');
    const link = path.join(dir, 'link-stub');
    await symlink(stub, link);
    const r = await verifyFile(link, { isWindowsTarget: false });
    expect(r.ok).toBe(false);
  });
  it('accepts a symlink to a real 2 MiB ELF 0755 binary', async () => {
    const real = path.join(dir, 'real');
    const buf = Buffer.alloc(2 * MiB);
    Buffer.from(ELF).copy(buf, 0);
    await writeFile(real, buf);
    await chmod(real, 0o755);
    const link = path.join(dir, 'link-real');
    await symlink(real, link);
    const r = await verifyFile(link, { isWindowsTarget: false });
    expect(r.ok).toBe(true);
  });
});
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `pnpm vitest run src/__tests__/verify-sidecar.test.ts`
Expected: FAIL — cannot find module `../../scripts/verify-sidecar-lib.mjs`.

- [ ] **Step 3: Implement `scripts/verify-sidecar-lib.mjs`**

```js
// verify-sidecar-lib.mjs — pure library. NO process.exit here: vitest
// imports this module, and verifyFile returns results instead of exiting
// (the CLI owns exit behavior). Spec §1 as amended (Opus M1/M3, GLM).
import { stat, open } from 'node:fs/promises';
import path from 'node:path';

export const MIN_SIZE = 1024 * 1024; // 1 MiB

const MAGIC = [
  { bytes: [0x7f, 0x45, 0x4c, 0x46], name: 'ELF' },
  { bytes: [0xfe, 0xed, 0xfa, 0xce], name: 'Mach-O thin (be)' },
  { bytes: [0xce, 0xfa, 0xed, 0xfe], name: 'Mach-O thin (le)' },
  { bytes: [0xfe, 0xed, 0xfa, 0xcf], name: 'Mach-O 64 (be)' },
  { bytes: [0xcf, 0xfa, 0xed, 0xfe], name: 'Mach-O 64 (le)' },
  { bytes: [0xca, 0xfe, 0xba, 0xbe], name: 'Mach-O fat' },
  { bytes: [0xca, 0xfe, 0xba, 0xbf], name: 'Mach-O fat64' },
  { bytes: [0x4d, 0x5a], name: 'PE (MZ)' },
];

export function checkSidecarBinary({ size, mode, head, isWindowsTarget }) {
  if (!Number.isFinite(size) || size < MIN_SIZE) {
    return { ok: false, reason: `size ${size} bytes < ${MIN_SIZE} (placeholder?)` };
  }
  const h = Array.from(head ?? []);
  const magic = MAGIC.find((m) => m.bytes.every((b, i) => h[i] === b));
  if (!magic) {
    return { ok: false, reason: `header ${h.map((b) => b.toString(16).padStart(2, '0')).join(' ')} is not a known executable format` };
  }
  // Missing/unknown mode REJECTS (fail closed, Opus m6). Windows targets
  // have no exec bit, so the check is skipped there.
  if (!isWindowsTarget && (typeof mode !== 'number' || (mode & 0o111) === 0)) {
    return { ok: false, reason: typeof mode === 'number'
      ? `mode ${(mode & 0o777).toString(8)} has no exec bit`
      : `mode unknown (${String(mode)}) — cannot confirm executable` };
  }
  return { ok: true };
}

export function resolveSidecarPath({ envTriple, hostTriple, repoRoot, binariesDir = 'src-tauri/binaries' }) {
  const triple = (envTriple && String(envTriple).trim()) || hostTriple;
  if (!triple) throw new Error('no target triple: TAURI_ENV_TARGET_TRIPLE unset and no host triple given');
  const exe = triple.includes('windows') ? '.exe' : '';
  return path.resolve(repoRoot, binariesDir, `curated-thoughts-mcp-${triple}${exe}`);
}

export async function verifyFile(filePath, { isWindowsTarget = false } = {}) {
  let st;
  try {
    st = await stat(filePath); // stat NOT lstat: follow symlinks (GLM case)
  } catch (err) {
    return { ok: false, reason: `cannot stat: ${err.message}` };
  }
  if (!st.isFile()) {
    return { ok: false, reason: `not a regular file` };
  }
  let head;
  try {
    const handle = await open(filePath, 'r');
    try {
      const buf = Buffer.alloc(4);
      await handle.read(buf, 0, 4, 0);
      head = [...buf];
    } finally {
      await handle.close();
    }
  } catch (err) {
    return { ok: false, reason: `cannot read: ${err.message}` };
  }
  const result = checkSidecarBinary({ size: st.size, mode: st.mode, head, isWindowsTarget });
  return result.ok ? { ok: true, size: st.size } : result;
}
```

- [ ] **Step 4: Implement `scripts/verify-sidecar.mjs` (CLI)**

```js
#!/usr/bin/env node
// verify-sidecar.mjs — CLI entry. ALWAYS runs the check when executed;
// all logic lives in verify-sidecar-lib.mjs (which never exits).
// Usage: verify-sidecar.mjs [path]
//   no path: bundle-hook mode — checks the sidecar for the triple being
//   bundled (TAURI_ENV_TARGET_TRIPLE, else rustc -vV host).
//   with path: checks that file (non-Windows unless it ends in .exe).
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { verifyFile, resolveSidecarPath } from './verify-sidecar-lib.mjs';

function fail(filePath, reason) {
  console.error(`ERROR: sidecar verification failed for ${filePath}`);
  console.error(`  ${reason}`);
  console.error(`Fix: run scripts/build-local-bundle.sh (local) or check build.yml's "Build MCP sidecar" step (CI).`);
  process.exit(1);
}

async function hostTripleFromRustc() {
  try {
    const { stdout } = await promisify(execFile)('rustc', ['-vV']);
    const line = stdout.split('\n').find((l) => l.startsWith('host:'));
    return line ? line.slice('host:'.length).trim() : undefined;
  } catch {
    return undefined; // caller fails closed with the Fix line (Opus m5)
  }
}

async function main() {
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const explicit = process.argv[2];
  if (explicit) {
    const p = path.resolve(explicit);
    const r = await verifyFile(p, { isWindowsTarget: p.endsWith('.exe') });
    if (!r.ok) fail(p, r.reason);
    console.error(`verify-sidecar: OK (${p}, ${r.size} bytes)`);
    return;
  }
  const envTriple = process.env.TAURI_ENV_TARGET_TRIPLE;
  const triple = (envTriple && envTriple.trim()) || (await hostTripleFromRustc());
  if (!triple) fail('<sidecar>', 'TAURI_ENV_TARGET_TRIPLE unset and rustc -vV unavailable/unparsed');
  const sidecar = resolveSidecarPath({ envTriple: triple, hostTriple: triple, repoRoot });
  const r = await verifyFile(sidecar, { isWindowsTarget: triple.includes('windows') });
  if (!r.ok) fail(sidecar, r.reason);
  console.error(`verify-sidecar: OK (${sidecar}, ${r.size} bytes)`);
}

await main();
```

- [ ] **Step 5: Run tests, verify pass**

Run: `pnpm vitest run src/__tests__/verify-sidecar.test.ts`
Expected: PASS (all cases).

- [ ] **Step 6: Manual CLI sanity (exit codes via PIPESTATUS, Opus m1)**

Run:
```bash
node scripts/verify-sidecar.mjs /usr/bin/curated-thoughts-mcp; echo "exit=$?"   # real 2.16.1 sidecar → OK, exit 0
node scripts/verify-sidecar.mjs /bin/true; echo "exit=$?"                       # tiny real ELF → size reject, exit 1
node scripts/verify-sidecar.mjs; echo "exit=$?"                                 # hook mode, placeholder present → exit 1, names placeholder + Fix line
ln -sf scripts/verify-sidecar.mjs /tmp/vs-link && node /tmp/vs-link /bin/true; echo "exit=$?"   # symlinked CLI still runs (no silent pass, Opus M1)
```
Expected: first 0; the rest 1 with the Fix line. The symlinked invocation must NOT exit 0.

- [ ] **Step 7: Commit**

```bash
git add scripts/verify-sidecar-lib.mjs scripts/verify-sidecar.mjs src/__tests__/verify-sidecar.test.ts
git commit -m "feat(scripts): sidecar verifier — pure lib + thin CLI, fail-closed, vitest-covered"
```

### Task 2: Wire the bundle hook

**Files:**
- Modify: `src-tauri/tauri.conf.json` (build section)

**Interfaces:**
- Consumes: Task 1's CLI (hook mode).
- Produces: every `tauri build` (with bundling) runs the guard.

- [ ] **Step 1: Edit `src-tauri/tauri.conf.json`**

In the `build` object, add:

```json
"beforeBundleCommand": "node scripts/verify-sidecar.mjs"
```

Keep `beforeDevCommand`, `devUrl`, `beforeBuildCommand`, `frontendDist` untouched.

- [ ] **Step 2: Validate JSON + schema sanity**

Run: `python3 -c "import json; json.load(open('src-tauri/tauri.conf.json')); print('json ok')" && pnpm tauri --version`
Expected: `json ok`, CLI prints 2.x.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/tauri.conf.json
git commit -m "feat(build): beforeBundleCommand runs the sidecar guard before every bundle"
```

---

### Task 3: Prove the guard fails closed (Linux local)

**Files:**
- none created — verification only (results recorded in PR body later)

**Interfaces:**
- Consumes: Tasks 1–2.
- Produces: evidence for the PR body + spec §7 note that Linux behavior is proven.

- [ ] **Step 1: Ensure the placeholder is in place**

Run: `ls -la src-tauri/binaries/` — placeholder must exist and be < 1 MiB (it is 10 bytes; if a previous task replaced it with the real binary, temporarily move the real one aside and `touch` a placeholder, restoring after Step 3).

- [ ] **Step 2: Run a bare bundle build**

Run: `pnpm tauri build --bundles deb 2>&1 | tail -15; echo "exit=${PIPESTATUS[0]}"`
Expected: FAILS during the bundle phase with the verifier's message naming the placeholder and the fix line; exit code non-zero. (Frontend build + cargo build still run first — that is expected and takes minutes.)

- [ ] **Step 3: Confirm no .deb was produced**

Run: `ls -t target/release/bundle/deb/*.deb 2>/dev/null | head -1` and compare mtime — no new .deb.

- [ ] **Step 4: Record evidence**

Save the failing tail to `/tmp/ct229-guard-fails.txt` for the PR body.

---

### Task 4: `scripts/build-local-bundle.sh` — the wrapper

**Files:**
- Create: `scripts/build-local-bundle.sh` (mode 0755)

**Interfaces:**
- Consumes: Task 1 CLI (explicit-path mode), `tools/smoke_test_mcp_sidecar.sh` (stages its own HOME — safe locally), CI's recipe order (load-bearing: sidecar build, then app build via `pnpm tauri build`).
- Produces: a local `.deb`/`.app` bundle whose sidecar is the real `--features mcp-server` build.

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
# build-local-bundle.sh — build a local bundle with a REAL MCP sidecar.
# Usage: build-local-bundle.sh [bundles]   (default: deb on Linux, app on macOS)
# Runs CI's sidecar recipe for the HOST triple, then bundles.
# Refuses Windows and universal targets (CI-only; spec §3).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# --- tool checks (spec §3) ---
for tool in cargo rustc node jq pnpm; do
  command -v "$tool" >/dev/null || { echo "ERROR: $tool not found in PATH" >&2; exit 1; }
done
python3 --version >/dev/null 2>&1 || { echo "ERROR: python3 not found (needed by the smoke test)" >&2; exit 1; }

TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
case "$TRIPLE" in
  *windows*) echo "ERROR: Windows bundles are CI-only (no bash recipe). Use the release workflow." >&2; exit 1 ;;
  *darwin*)
    case "$TRIPLE" in
      *universal*) echo "ERROR: universal macOS bundles are CI-only. Build for your host triple." >&2; exit 1 ;;
    esac
    BUNDLES="${1:-app}" ;;
  *) BUNDLES="${1:-deb}" ;;
esac

DEST="src-tauri/binaries/curated-thoughts-mcp-$TRIPLE"
mkdir -p src-tauri/binaries
[ -e "$DEST" ] || touch "$DEST"   # placeholder satisfies tauri-build; never truncate a real binary
# (No universal branch: rustc -vV never reports a universal host triple, so
#  this recipe physically cannot produce one — Opus plan-review m4.)

echo "== Building MCP sidecar (--features mcp-server) for $TRIPLE =="
cargo build --release --manifest-path src-tauri/Cargo.toml --features mcp-server --bin curated-thoughts

TARGET_DIR="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)"
cp "$TARGET_DIR/release/curated-thoughts" "$DEST"
chmod +x "$DEST"

echo "== Verifying + smoke-testing the sidecar =="
node scripts/verify-sidecar.mjs "$DEST"
tools/smoke_test_mcp_sidecar.sh "$DEST"

echo "== Bundling ($BUNDLES) — the hook re-verifies independently =="
pnpm tauri build --bundles "$BUNDLES"

echo "== Done. Bundle sidecar verified; hook guard passed. =="
```

- [ ] **Step 2: chmod +x and shellcheck**

Run: `chmod +x scripts/build-local-bundle.sh && bash -n scripts/build-local-bundle.sh && echo ok`
Expected: `ok`. (If `shellcheck` is installed, run it; fix only real findings.)

- [ ] **Step 3: Commit**

```bash
git add scripts/build-local-bundle.sh
git commit -m "feat(scripts): build-local-bundle.sh — CI sidecar recipe + guarded bundle in one command"
```

---

### Task 5: `install-ct.sh` gate + error-message fix

**Files:**
- Modify: `scripts/install-ct.sh:17` (message) and after line 21 (gate)

**Interfaces:**
- Consumes: Task 1 CLI (explicit-path mode); `dpkg-deb -x`.
- Produces: installer refuses a broken-sidecar `.deb` before `sudo dpkg -i`.

- [ ] **Step 1: Edit the no-deb error (line 17)**

Replace:

```bash
    echo "ERROR: no .deb found under ${DEB_DIRS[*]} — run 'pnpm tauri build --bundles deb' first" >&2
```

with:

```bash
    echo "ERROR: no .deb found under ${DEB_DIRS[*]} — run 'scripts/build-local-bundle.sh' first" >&2
```

- [ ] **Step 2: Insert the gate after line 21 (`[[ -f "$DEB" ]] || ...`)**

```bash
# Sidecar gate: refuse a .deb whose packaged sidecar is a placeholder/broken
# (spec §4; reuses the repo verifier so the rules cannot drift).
TMP_EXTRACT="$(mktemp -d)"
trap 'rm -rf "$TMP_EXTRACT"' EXIT
dpkg-deb -x "$DEB" "$TMP_EXTRACT"
SIDECAR="$TMP_EXTRACT/usr/bin/curated-thoughts-mcp"
if [[ ! -e "$SIDECAR" ]]; then
  echo "ERROR: refusing to install: no sidecar in $DEB" >&2
  exit 1
fi
command -v node >/dev/null 2>&1 || { echo "ERROR: node is required for the sidecar gate but was not found in PATH" >&2; exit 1; }
node "$REPO_ROOT/scripts/verify-sidecar.mjs" "$SIDECAR" || {
  echo "ERROR: refusing to install: broken sidecar in $DEB" >&2
  exit 1
}
```

- [ ] **Step 3: Prove both paths — with shimmed `sudo`/`pkill` (Opus M2)**

The PASS path reaches the script's `sudo dpkg -i` and `pkill` lines. Running
them for real risks an actual install (cached sudo credentials / NOPASSWD)
and kills the GUI app **and any running MCP sidecars** (`pkill -f
'/usr/bin/curated-thoughts'` matches the sidecar too). Shim both:

```bash
mkdir -p /tmp/ct229-shim
printf '#!/bin/sh\necho "SHIM sudo $*"\n' > /tmp/ct229-shim/sudo
printf '#!/bin/sh\necho "SHIM pkill $* (skipped)"\n' > /tmp/ct229-shim/pkill
chmod +x /tmp/ct229-shim/sudo /tmp/ct229-shim/pkill
```

```bash
# REFUSE path — bad 2.15.1 local deb (10-byte sidecar):
PATH="/tmp/ct229-shim:$PATH" bash scripts/install-ct.sh "$HOME/code/github/equationalapplications/curated-thoughts/target/release/bundle/deb/Curated Thoughts_2.15.1_amd64.deb"; echo "exit=$?"
```
Expected: `ERROR: refusing to install: broken sidecar`, `exit=1`, **no shim
output** (gate fires before anything else).

```bash
# PASS path — official 2.16.1 deb:
PATH="/tmp/ct229-shim:$PATH" bash scripts/install-ct.sh /tmp/ct-verify/Curated.Thoughts_2.16.1_amd64.deb; echo "exit=$?"
```
Expected: verifier OK line, then `SHIM pkill ...` and `SHIM sudo dpkg -i ...`
lines, `== Installed:` from the dpkg-query (real, read-only), config sanity,
`exit=0`. **Nothing installed, nothing killed.**

Delete the shims afterwards: `rm -rf /tmp/ct229-shim`.

- [ ] **Step 4: Commit**

```bash
git add scripts/install-ct.sh
git commit -m "feat(scripts): install-ct.sh refuses .debs with broken sidecars; error message points at the wrapper"
```

---

### Task 6: README

**Files:**
- Modify: `README.md` (placeholder block ~line 85-91, build command ~line 100-103)

**Interfaces:**
- Consumes: Tasks 1–4 (what to tell users to run).
- Produces: docs that cannot lead a newcomer into a broken bundle.

- [ ] **Step 1: Edit the placeholder block (after the `touch` command, before the closing fence)**

Insert after line 90's `touch` line (keep the Windows `.exe` note):

```markdown
This placeholder is **for dev and test only** (`pnpm tauri dev`,
`cargo test`, clippy). Never build a bundle on top of it — the build fails
closed by design. To produce an installable bundle, use the wrapper:

```bash
# Build a bundle with the real MCP sidecar (Linux: .deb; macOS: .app)
scripts/build-local-bundle.sh
```
```

- [ ] **Step 2: Replace the bare build command (line 102)**

Replace `pnpm tauri build` with:

```markdown
scripts/build-local-bundle.sh [deb|app|dmg|rpm]
```

Add one sentence after it: bundles must go through the wrapper so the MCP
sidecar is the real `mcp-server` build; **macOS universal bundles are CI-only**
(the wrapper refuses them) — Apple Silicon developers build for their host
triple.

- [ ] **Step 2b: Update `CONTRIBUTORS.md:48` (Opus m2)**

The line "Build locally: `pnpm run tauri build`" now fails closed on a
placeholder checkout. Change it to `scripts/build-local-bundle.sh` (same
wording rationale as the README).

- [ ] **Step 3: Commit**

```bash
git add README.md CONTRIBUTORS.md
git commit -m "docs(readme): bundles go through build-local-bundle.sh; placeholder is dev-only"
```

---

### Task 7: Full test-plan pass (spec §6) + push

**Files:** none created; this is the verification gate before review.

- [ ] **Step 1: Automated suite**

Run: `pnpm vitest run src/__tests__/verify-sidecar.test.ts && pnpm typecheck && pnpm lint && pnpm test`
Expected: all green (`pnpm test` runs the whole vitest suite including the new file).

- [ ] **Step 2: Manual matrix — re-confirm each item, check off in PR body**

From spec §6 manual list: guard fails on placeholder (Task 3 evidence), wrapper produces a passing bundle (run `scripts/build-local-bundle.sh` — takes a while; the smoke test runs the onboarding+MCP handshake in a staged HOME), **and the packaged sidecar is smoke-tested from the extracted .deb** (Opus M4 — the wrapper's smoke test proves the source file, not what the bundler packaged):

```bash
DEB=$(ls -t target/release/bundle/deb/*.deb | head -1)
X=$(mktemp -d) && dpkg-deb -x "$DEB" "$X"
tools/smoke_test_mcp_sidecar.sh "$X/usr/bin/curated-thoughts-mcp" && echo "PACKAGED SIDECAR OK"
rm -rf "$X"
```
Expected: `PACKAGED SIDECAR OK`. `install-ct.sh` rejects placeholder debs (Task 5 evidence).

- [ ] **Step 3: Push**

```bash
git push origin fix/local-build-sidecar-packaging
```

---

## Plan-level notes for executors

- Work on branch `fix/local-build-sidecar-packaging` (already checked out at `c9beb9c`). No worktree — single implementer sequence.
- Do NOT run `sudo dpkg -i` at any point (Task 5's PASS path ends at the sudo prompt by design).
- The wrapper's full run (Task 7 Step 2) compiles the Rust workspace twice and takes 10+ minutes on this ThinkPad; use generous timeouts (≥ 1200s), never interrupt mid-`cargo`.
- `TAURI_ENV_TARGET_TRIPLE` is exported to the hook by CLI 2.11.4 (empirically confirmed); the verifier's rustc fallback is for manual runs only.
- Spec §7's scratch-branch release-CI verification is a SEPARATE workstream (dispatch-only workflow on a scratch branch, macOS + Windows) — not part of these tasks. **The PR must NOT merge until §7's scratch-branch checkboxes are ticked** (Opus m7: if `TAURI_ENV_TARGET_TRIPLE` were an arch triple under `--target universal-apple-darwin`, the hook would check the 0-byte arch placeholders `build.yml` creates and the macOS release would fail — safe direction, but it must be observed before merge, not after).
