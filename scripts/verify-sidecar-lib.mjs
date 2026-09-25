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
