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
