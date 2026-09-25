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
