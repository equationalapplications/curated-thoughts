import { describe, it, expect, vi, afterEach } from 'vitest';
import { guardListen, safeUnlisten, type UnlistenFn } from '../lib/events';

describe('guardListen', () => {
  afterEach(() => vi.restoreAllMocks());

  it('logs one warning with context when the subscription rejects', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const boom = new Error('listen failed');
    const p = guardListen(Promise.reject(boom), 'provider-loading');
    await p.catch(() => {});
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn).toHaveBeenCalledWith('[events] listen failed (provider-loading)', boom);
  });

  it('returns the SAME promise (identity preserved for cleanup)', () => {
    const inner: Promise<UnlistenFn> = Promise.resolve(() => {});
    expect(guardListen(inner, 'ctx')).toBe(inner);
  });

  it('never leaves an unhandled rejection even if nobody awaits the result', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    guardListen(Promise.reject(new Error('x')), 'ctx');
    await new Promise((r) => setTimeout(r, 0));
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it('stays silent when the subscription resolves', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await guardListen(Promise.resolve(() => {}), 'ctx');
    expect(warn).not.toHaveBeenCalled();
  });

  it('result still works with safeUnlisten after a rejection', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await expect(
      safeUnlisten(guardListen(Promise.reject(new Error('x')), 'ctx')),
    ).resolves.toBeUndefined();
    expect(warn).toHaveBeenCalledTimes(1); // guardListen logged; safeUnlisten swallowed
  });
});
