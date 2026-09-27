import { describe, it, expect, vi, afterEach } from 'vitest';
import { safeUnlisten } from '../lib/events';

// Tauri's listen() resolves before the webview has registered the listener,
// so an immediate unlisten() throws inside unregisterListener.
const registrationRace = () =>
  new TypeError("undefined is not an object (evaluating 'listeners[eventId].handlerId')");

describe('safeUnlisten', () => {
  afterEach(() => vi.restoreAllMocks());

  it('retries until the listener registration has landed', async () => {
    const unlisten = vi
      .fn()
      .mockRejectedValueOnce(registrationRace())
      .mockRejectedValueOnce(registrationRace())
      .mockResolvedValue(undefined);

    await safeUnlisten(Promise.resolve(unlisten));

    expect(unlisten).toHaveBeenCalledTimes(3);
  });

  it('accepts an already-resolved unlisten function', async () => {
    const unlisten = vi.fn().mockResolvedValue(undefined);
    await safeUnlisten(unlisten);
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it('backs off, then gives up quietly instead of leaving an unhandled rejection', async () => {
    vi.useFakeTimers();
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const unlisten = vi.fn().mockRejectedValue(registrationRace());

    const done = safeUnlisten(unlisten);
    await vi.runAllTimersAsync();
    await expect(done).resolves.toBeUndefined();

    expect(unlisten).toHaveBeenCalledTimes(11);
    expect(warn).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });

  it('ignores a listen() that never subscribed', async () => {
    await expect(safeUnlisten(Promise.reject(new Error('listen failed')))).resolves.toBeUndefined();
  });

  it('is a no-op for undefined', async () => {
    await expect(safeUnlisten(undefined)).resolves.toBeUndefined();
  });
});
