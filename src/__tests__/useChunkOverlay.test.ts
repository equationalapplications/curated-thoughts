import { renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { useChunkOverlay, type BlockLineMap } from "../hooks/useChunkOverlay";

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

/** Build a container with one block element carrying `data-id`, plus a
 * matching lineMap entry. Returns the container (NOT appended to the
 * document — the test appends explicitly so cleanup is local). */
function makeContainerWithBlock(
  blockId: string,
  startLine: number,
  endLine: number,
): { container: HTMLElement; lineMap: BlockLineMap } {
  const container = document.createElement("div");
  const block = document.createElement("div");
  block.setAttribute("data-id", blockId);
  container.appendChild(block);
  // give the block a measurable size for getBoundingClientRect to work
  Object.defineProperty(block, "getBoundingClientRect", {
    value: () => ({
      top: 100,
      bottom: 130,
      left: 0,
      right: 600,
      width: 600,
      height: 30,
      x: 0,
      y: 100,
      toJSON: () => ({}),
    }),
  });
  const lineMap: BlockLineMap = new Map([[blockId, [startLine, endLine]]]);
  return { container, lineMap };
}

test("calls resolveChunkOverlay on mount with the path and hash", async () => {
  vi.mocked(invoke).mockResolvedValue({ startLine: 10, endLine: 15 });
  const { container, lineMap } = makeContainerWithBlock("block-1", 5, 20);
  document.body.appendChild(container);
  try {
    const { result } = renderHook(() =>
      useChunkOverlay("documents/notes.md", "abc123", container, lineMap),
    );
    await waitFor(() => expect(result.current.status).toBe("visible"));
    expect(invoke).toHaveBeenCalledWith(
      "resolve_chunk_overlay",
      { path: "documents/notes.md", hash: "abc123" },
    );
  } finally {
    container.remove();
  }
});

test("re-fetches when anchorChunkId changes", async () => {
  vi.mocked(invoke).mockResolvedValue({ startLine: 10, endLine: 15 });
  const { container, lineMap } = makeContainerWithBlock("block-1", 5, 20);
  document.body.appendChild(container);
  try {
    const { result, rerender } = renderHook(
      ({ hash }) =>
        useChunkOverlay("documents/notes.md", hash, container, lineMap),
      { initialProps: { hash: "abc" } },
    );
    await waitFor(() => expect(result.current.status).toBe("visible"));
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({ startLine: 20, endLine: 25 });
    rerender({ hash: "def" });
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "resolve_chunk_overlay",
        { path: "documents/notes.md", hash: "def" },
      ),
    );
  } finally {
    container.remove();
  }
});

test("falls back to source-moved-notice on invoke error", async () => {
  vi.mocked(invoke).mockRejectedValue(new Error("IPC down"));
  const { container, lineMap } = makeContainerWithBlock("block-1", 5, 20);
  document.body.appendChild(container);
  try {
    const { result } = renderHook(() =>
      useChunkOverlay("documents/notes.md", "abc123", container, lineMap),
    );
    await waitFor(() => expect(result.current.status).toBe("source-moved"));
  } finally {
    container.remove();
  }
});

test("returns idle for empty hash without calling invoke", async () => {
  const { container, lineMap } = makeContainerWithBlock("block-1", 5, 20);
  document.body.appendChild(container);
  try {
    const { result } = renderHook(() =>
      useChunkOverlay("documents/notes.md", null, container, lineMap),
    );
    expect(result.current.status).toBe("idle");
    expect(invoke).not.toHaveBeenCalled();
  } finally {
    container.remove();
  }
});
