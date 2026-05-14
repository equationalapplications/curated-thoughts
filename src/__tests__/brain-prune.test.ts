import * as React from 'react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { startAutoMaintenance } from '../lib/wiki';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useWikiStatus } from '../hooks/useWikiStatus';
import { MaintenanceDashboard } from '../components/settings/MaintenanceDashboard';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(),
}));

vi.mock('../hooks/useWikiStatus', () => ({
  useWikiStatus: vi.fn(),
}));

describe('startAutoMaintenance', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    vi.spyOn(globalThis, 'setInterval');
    vi.spyOn(globalThis, 'clearInterval');
    vi.mocked(invoke).mockResolvedValue(undefined);
    vi.mocked(listen).mockResolvedValue(() => {});
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it('runs prune immediately and schedules a daily prune', async () => {
    const cleanup = startAutoMaintenance();

    expect(invoke).toHaveBeenCalledWith('run_wiki_prune');
    expect(setInterval).toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(24 * 60 * 60 * 1000);
    expect(invoke).toHaveBeenCalledTimes(2);

    cleanup();
  });

  it('cancels the scheduled prune when cleanup is called', async () => {
    const cleanup = startAutoMaintenance();
    expect(invoke).toHaveBeenCalledTimes(1);

    cleanup();
    await vi.advanceTimersByTimeAsync(24 * 60 * 60 * 1000);

    expect(invoke).toHaveBeenCalledTimes(1);
    expect(clearInterval).toHaveBeenCalled();
  });

  it('does not throw when the prune command fails', async () => {
    vi.mocked(invoke).mockRejectedValueOnce(new Error('prune failed'));
    const cleanup = startAutoMaintenance();

    await vi.advanceTimersByTimeAsync(0);
    cleanup();

    expect(invoke).toHaveBeenCalledWith('run_wiki_prune');
  });

  it('skips scheduled prune while the system is busy', async () => {
    type StatusEvent = {
      payload: {
        ingesting: boolean;
        librarian: boolean;
        healing: boolean;
        pruning: boolean;
        forgetting: boolean;
      };
    };
    let statusCallback: (event: StatusEvent) => void = () => {};
    vi.mocked(listen).mockImplementation(async (_event, cb: (event: StatusEvent) => void) => {
      statusCallback = cb;
      return () => {};
    });

    const cleanup = startAutoMaintenance();
    statusCallback({
      payload: { ingesting: true, librarian: false, healing: false, pruning: false, forgetting: false },
    });

    await vi.advanceTimersByTimeAsync(24 * 60 * 60 * 1000);

    expect(invoke).toHaveBeenCalledTimes(1);

    cleanup();
  });
});

describe('MaintenanceDashboard', () => {
  beforeEach(() => {
    vi.mocked(useWikiStatus).mockReturnValue({
      ingesting: false,
      librarian: false,
      healing: false,
      pruning: false,
      forgetting: false,
      busy: false,
      activeJob: 'idle',
      activeJobLabel: null,
    });
  });

  it('calls run_wiki_prune when the Prune Trash button is clicked', async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    render(React.createElement(MaintenanceDashboard));

    const pruneButton = screen.getByRole('button', { name: /Prune Trash/i });
    fireEvent.click(pruneButton);

    expect(invoke).toHaveBeenCalledWith('run_wiki_prune');
  });
});
