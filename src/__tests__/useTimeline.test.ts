import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest';
import { renderHook, act, waitFor } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import type { TimelineEvent, TimelineFilter } from '../lib/tauri';
import { useTimeline } from '../hooks/useTimeline';

vi.mock('@tauri-apps/api/core');

const MOCK_EVENT_1: TimelineEvent = {
  id: 'evt_1',
  kind: 'synthesized',
  summary: 'Event 1',
  entity_id: 'ent_1',
  entity_name: 'Entity 1',
  doc_path: null,
  raw_type: 'event_type_1',
  client: null,
  created_at_ms: 1000,
};

const MOCK_EVENT_2: TimelineEvent = {
  id: 'evt_2',
  kind: 'approved',
  summary: 'Event 2',
  entity_id: 'ent_2',
  entity_name: 'Entity 2',
  doc_path: '/path/to/doc',
  raw_type: 'event_type_2',
  client: 'web',
  created_at_ms: 2000,
};

describe('useTimeline', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('calls listEvents with filter on initial load and sets events', async () => {
    const filter: TimelineFilter = { kinds: ['synthesized', 'approved'], limit: 10 };
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === 'list_events_cmd') {
        return Promise.resolve([MOCK_EVENT_1]);
      }
      return Promise.resolve(null);
    });

    const { result } = renderHook(() => useTimeline(filter));
    await waitFor(() => expect(result.current.events.length).toBeGreaterThan(0));

    expect(result.current.events).toEqual([MOCK_EVENT_1]);
    expect(result.current.error).toBeNull();
    expect(invoke).toHaveBeenCalledWith('list_events_cmd', { filter });
  });

  it('polls listEvents at POLL_MS intervals', async () => {
    const filter: TimelineFilter = {};
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === 'list_events_cmd') {
        return Promise.resolve([MOCK_EVENT_1]);
      }
      return Promise.resolve(null);
    });

    const { result } = renderHook(() => useTimeline(filter));
    await waitFor(() => expect(result.current.events.length).toBeGreaterThan(0));

    expect(invoke).toHaveBeenCalledTimes(1);

    // Advance time by 5000ms (POLL_MS)
    await act(async () => {
      vi.advanceTimersByTimeAsync(5000);
    });

    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it('sets error on listEvents failure but preserves previous events', async () => {
    const filter: TimelineFilter = {};
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === 'list_events_cmd') {
        return Promise.resolve([MOCK_EVENT_1]);
      }
      return Promise.resolve(null);
    });

    const { result } = renderHook(() => useTimeline(filter));
    await waitFor(() => expect(result.current.events.length).toBeGreaterThan(0));

    expect(result.current.events).toEqual([MOCK_EVENT_1]);
    expect(result.current.error).toBeNull();

    // Mock subsequent calls to fail
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === 'list_events_cmd') {
        return Promise.reject(new Error('Connection failed'));
      }
      return Promise.resolve(null);
    });

    await act(async () => {
      vi.advanceTimersByTimeAsync(5000);
    });

    await waitFor(() => expect(result.current.error).not.toBeNull());

    expect(result.current.error).toBe('Timeline is temporarily unavailable.');
    expect(result.current.events).toEqual([MOCK_EVENT_1]);
  });

  it('allows manual refresh that re-fetches without waiting for interval', async () => {
    const filter: TimelineFilter = {};
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === 'list_events_cmd') {
        return Promise.resolve([MOCK_EVENT_1]);
      }
      return Promise.resolve(null);
    });

    const { result } = renderHook(() => useTimeline(filter));
    await waitFor(() => expect(result.current.events.length).toBeGreaterThan(0));

    expect(invoke).toHaveBeenCalledTimes(1);

    // Mock subsequent calls to return a different event
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === 'list_events_cmd') {
        return Promise.resolve([MOCK_EVENT_1, MOCK_EVENT_2]);
      }
      return Promise.resolve(null);
    });

    // Manually call refresh
    await act(async () => {
      result.current.refresh();
    });

    await waitFor(() => expect(result.current.events).toHaveLength(2));

    expect(result.current.events).toEqual([MOCK_EVENT_1, MOCK_EVENT_2]);
    expect(invoke).toHaveBeenCalledTimes(2);
  });
});
