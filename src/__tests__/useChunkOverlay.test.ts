import { renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { useChunkOverlay } from "../hooks/useChunkOverlay";

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

test("calls resolveChunkOverlay on mount with the path and hash", async () => {
  vi.mocked(invoke).mockResolvedValue({ startLine: 10, endLine: 15 });
  const { result } = renderHook(() =>
    useChunkOverlay("documents/notes.md", "abc123"),
  );
  await waitFor(() => expect(result.current.status).toBe("visible"));
  expect(invoke).toHaveBeenCalledWith(
    "resolve_chunk_overlay",
    { path: "documents/notes.md", hash: "abc123" },
  );
});

test("re-fetches when anchorChunkId changes", async () => {
  vi.mocked(invoke).mockResolvedValue({ startLine: 10, endLine: 15 });
  const { result, rerender } = renderHook(
    ({ hash }) => useChunkOverlay("documents/notes.md", hash),
    { initialProps: { hash: "abc" } },
  );
  await waitFor(() => expect(result.current.status).toBe("visible"));
  vi.mocked(invoke).mockClear();
  vi.mocked(invoke).mockResolvedValue({ startLine: 20, endLine: 25 });
  rerender({ hash: "def" });
  await waitFor(() => expect(invoke).toHaveBeenCalledWith(
    "resolve_chunk_overlay",
    { path: "documents/notes.md", hash: "def" },
  ));
});

test("falls back to source-moved-notice on invoke error", async () => {
  vi.mocked(invoke).mockRejectedValue(new Error("IPC down"));
  const { result } = renderHook(() =>
    useChunkOverlay("documents/notes.md", "abc123"),
  );
  await waitFor(() => expect(result.current.status).toBe("source-moved"));
});

test("returns idle for empty hash without calling invoke", async () => {
  const { result } = renderHook(() =>
    useChunkOverlay("documents/notes.md", null),
  );
  expect(result.current.status).toBe("idle");
  expect(invoke).not.toHaveBeenCalled();
});
