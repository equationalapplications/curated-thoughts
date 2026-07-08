import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import { useTimeline } from '../hooks/useTimeline';

const SAMPLE_EVENTS = [
  { id: '1', kind: 'approved', summary: 'Approved fact', entity_id: null, entity_name: null, doc_path: null, raw_type: 'approved', client: null, created_at_ms: 1000 },
  { id: '2', kind: 'agent_access', summary: 'agent called tool', entity_id: null, entity_name: null, doc_path: null, raw_type: 'tool', client: 'test', created_at_ms: 2000 },
];

beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === 'list_events') return Promise.resolve(SAMPLE_EVENTS);
    return Promise.resolve(null);
  });
});

afterEach(() => {
  vi.useRealTimers();
});

it('loads events on mount', async () => {
  const { result } = renderHook(() => useTimeline({}));
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  expect(result.current.events).toEqual(SAMPLE_EVENTS);
  expect(result.current.error).toBeNull();
});

it('polls for new events', async () => {
  const { result } = renderHook(() => useTimeline({}));
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  expect(invoke).toHaveBeenCalledTimes(1);

  // Advance time by 5000ms (POLL_MS)
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });

  expect(invoke).toHaveBeenCalledTimes(2);
});

it('handles errors gracefully', async () => {
  vi.mocked(invoke).mockRejectedValue(new Error('Network error'));
  const { result } = renderHook(() => useTimeline({}));
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  expect(result.current.events).toEqual([]);
  expect(result.current.error).toBe('Timeline is temporarily unavailable.');
});

it('refreshes when filter changes', async () => {
  const { result, rerender } = renderHook(
    (filter) => useTimeline(filter),
    { initialProps: { kinds: ['approved'] } }
  );
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  expect(invoke).toHaveBeenCalledTimes(1);

  rerender({ kinds: ['agent_access'] });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  expect(invoke).toHaveBeenCalledTimes(2);
});
