import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import * as tauri from '../lib/tauri';
import { useMemoryRead } from '../hooks/useMemoryRead';

describe('useMemoryRead', () => {
  const semanticResult = {
    doc_path: '/Users/test/Vault/documents/notes.md',
    chunk_text: 'Hello world',
    chunk_position: 1,
    score: 0.4,
    start_line: 1,
    end_line: 1,
    symbol_name: null,
    strategy: 'semantic',
  };

  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('debounces query and returns weighted search results', async () => {
    const searchVault = vi.spyOn(tauri, 'searchVault').mockResolvedValue([semanticResult]);
    const getStructuralNeighbors = vi.spyOn(tauri, 'getStructuralNeighbors').mockResolvedValue([]);

    const { result } = renderHook(() => useMemoryRead('/Users/test/Vault'));

    act(() => {
      result.current.setQuery('hello');
    });

    expect(searchVault).not.toHaveBeenCalled();
    await act(async () => {
      vi.advanceTimersByTime(300);
    });

    expect(searchVault).toHaveBeenCalledWith('hello', 10);
    expect(getStructuralNeighbors).toHaveBeenCalledWith(semanticResult.doc_path, 1);
    expect(result.current.results[0].score).toBeCloseTo(0.6);
    expect(result.current.searching).toBe(false);
  });

  it('uses cache for repeated query values', async () => {
    const searchVault = vi.spyOn(tauri, 'searchVault').mockResolvedValue([semanticResult]);
    vi.spyOn(tauri, 'getStructuralNeighbors').mockResolvedValue([]);

    const { result } = renderHook(() => useMemoryRead('/Users/test/Vault'));

    act(() => {
      result.current.setQuery('hello');
    });
    await act(async () => vi.advanceTimersByTimeAsync(300));

    expect(searchVault).toHaveBeenCalledTimes(1);

    act(() => {
      result.current.setQuery('hello');
    });
    await act(async () => vi.advanceTimersByTimeAsync(300));

    expect(searchVault).toHaveBeenCalledTimes(1);
  });

  it('appends structural neighbors without duplicating semantic results', async () => {
    const searchVault = vi.spyOn(tauri, 'searchVault').mockResolvedValue([semanticResult]);
    vi.spyOn(tauri, 'getStructuralNeighbors').mockResolvedValue([
      {
        ...semanticResult,
        structural: true,
        rel_type: 'CALLS',
        score: 0.0,
      },
      {
        doc_path: '/Users/test/Vault/src/index.ts',
        chunk_text: 'Connected chunk',
        chunk_position: 2,
        score: 0.0,
        start_line: 10,
        end_line: 10,
        symbol_name: null,
        strategy: 'structural',
        structural: true,
        rel_type: 'CALLS',
      },
    ]);

    const { result } = renderHook(() => useMemoryRead('/Users/test/Vault'));

    act(() => {
      result.current.setQuery('hello');
    });
    await act(async () => vi.advanceTimersByTimeAsync(300));

    expect(result.current.results).toHaveLength(2);
    expect(result.current.results[1].structural).toBe(true);
  });

  it('expands structural neighbors for every semantic source path', async () => {
    const secondResult = {
      ...semanticResult,
      doc_path: '/Users/test/Vault/src/app.ts',
      chunk_position: 2,
    };
    const searchVault = vi.spyOn(tauri, 'searchVault').mockResolvedValue([
      semanticResult,
      secondResult,
    ]);
    const getStructuralNeighbors = vi.spyOn(tauri, 'getStructuralNeighbors').mockImplementation(
      (docPath: string) => {
        if (docPath.endsWith('notes.md')) {
          return Promise.resolve([
            {
              doc_path: '/Users/test/Vault/documents/notes.md',
              chunk_text: 'Connected neighbor 1',
              chunk_position: 3,
              score: 0.0,
              start_line: 5,
              end_line: 5,
              symbol_name: null,
              strategy: 'structural',
              structural: true,
              rel_type: 'CALLS',
            },
          ]);
        }
        if (docPath.endsWith('app.ts')) {
          return Promise.resolve([
            {
              doc_path: '/Users/test/Vault/src/app.ts',
              chunk_text: 'Connected neighbor 2',
              chunk_position: 4,
              score: 0.0,
              start_line: 2,
              end_line: 2,
              symbol_name: null,
              strategy: 'structural',
              structural: true,
              rel_type: 'CALLS',
            },
          ]);
        }
        return Promise.resolve([]);
      },
    );

    const { result } = renderHook(() => useMemoryRead('/Users/test/Vault'));

    act(() => {
      result.current.setQuery('hello');
    });
    await act(async () => vi.advanceTimersByTimeAsync(300));

    expect(getStructuralNeighbors).toHaveBeenCalledTimes(2);
    expect(getStructuralNeighbors).toHaveBeenCalledWith(semanticResult.doc_path, 1);
    expect(getStructuralNeighbors).toHaveBeenCalledWith(secondResult.doc_path, 1);
    expect(result.current.results).toHaveLength(4);
    expect(result.current.results.filter((r) => r.structural).length).toBe(2);
  });
});
