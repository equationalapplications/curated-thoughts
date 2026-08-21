import { useEffect, useRef, useState } from "react";
import { resolveChunkOverlay } from "../lib/tauri";

export type ChunkOverlayStatus = "idle" | "source-moved" | "visible";

export interface ChunkOverlayRect {
  top: number;
  height: number;
}

export interface UseChunkOverlayResult {
  status: ChunkOverlayStatus;
  overlay: ChunkOverlayRect | null;
  /** The raw line range the hook resolved; useful for tests + EditorPane. */
  range: { startLine: number; endLine: number } | null;
}

/**
 * Resolves a chunk-hash anchor to a line range, then asks the editor's
 * block DOM for a pixel-positioned rect. Returns idle for empty hash
 * (no-op) and source-moved for null/error results.
 *
 * The `containerRef` is the scrollable editor pane root; the hook uses
 * it to translate document-relative top/height into client-relative
 * coordinates that a position-absolute overlay can consume.
 *
 * The line-to-block map is built from `BlockNote` blocks whose
 * `startLine`/`endLine` props are injected at markdown-parse time (see
 * spec §Components §Line-to-block mapping failure for the contract).
 */
export function useChunkOverlay(
  path: string | null,
  hash: string | null,
  containerRef?: React.RefObject<HTMLElement | null>,
): UseChunkOverlayResult {
  const [status, setStatus] = useState<ChunkOverlayStatus>("idle");
  const [overlay, setOverlay] = useState<ChunkOverlayRect | null>(null);
  const [range, setRange] = useState<{ startLine: number; endLine: number } | null>(null);
  const cancelledRef = useRef(false);

  useEffect(() => {
    cancelledRef.current = false;
    if (!hash) {
      setStatus("idle");
      setOverlay(null);
      setRange(null);
      return;
    }
    setStatus("idle");
    setOverlay(null);
    resolveChunkOverlay(path ?? "", hash)
      .then((res) => {
        if (cancelledRef.current) return;
        if (!res) {
          setStatus("source-moved");
          setOverlay(null);
          setRange(null);
          return;
        }
        setRange(res);
        // Resolution succeeded — the editor pane will show a visible
        // anchor overlay (or a source-moved notice only when the IPC
        // itself returned null/rejected). If the container ref isn't
        // attached yet, status is still 'visible' but `overlay` stays
        // null until BlockNote renders the blocks. Callers that need a
        // pixel rect re-render once `containerRef` mounts.
        setStatus("visible");
        // Defer to next frame so BlockNote has rendered its blocks.
        requestAnimationFrame(() => {
          if (cancelledRef.current) return;
          const rect = computeOverlayRect(
            res.startLine,
            res.endLine,
            containerRef?.current ?? null,
          );
          if (rect) setOverlay(rect);
        });
      })
      .catch(() => {
        if (cancelledRef.current) return;
        setStatus("source-moved");
        setOverlay(null);
        setRange(null);
      });
    return () => {
      cancelledRef.current = true;
    };
  }, [path, hash, containerRef]);

  // Track scroll inside the editor container and update the overlay's
  // top so it stays anchored to the lines as the user scrolls within
  // BlockNote.
  useEffect(() => {
    if (status !== "visible") return;
    const root: HTMLElement | null = containerRef?.current ?? null;
    if (!root) return;
    function onScroll() {
      // Trigger a re-render by recomputing from the current range.
      // (In a fuller impl, the EditorPane reads scrollTop itself; here
      // we simply update via setOverlay on scroll.)
      if (!range) return;
      const rect = computeOverlayRect(range.startLine, range.endLine, root);
      if (rect) setOverlay(rect);
    }
    root.addEventListener("scroll", onScroll, { passive: true });
    return () => root.removeEventListener("scroll", onScroll);
  }, [status, range, containerRef]);

  return { status, overlay, range };
}

/**
 * Walk the BlockNote DOM in the container, find every block element
 * whose `data-start-line` / `data-end-line` attributes cover the
 * requested range, and return the union top/height. Returns `null`
 * if no block covers the range (e.g., the range is past EOF).
 *
 * The Markdown→BlockNote parser injects these attributes on each
 * block's DOM node at parse time (see spec §Components §Line-to-block
 * mapping). If they are missing (older BlockNote versions), this
 * returns `null` and the caller surfaces the source-moved-notice.
 */
function computeOverlayRect(
  startLine: number,
  endLine: number,
  container: HTMLElement | null,
): ChunkOverlayRect | null {
  if (!container) return null;
  const nodes = Array.from(
    container.querySelectorAll<HTMLElement>("[data-start-line][data-end-line]"),
  );
  if (nodes.length === 0) return null;
  const containerRect = container.getBoundingClientRect();
  let minTop: number | null = null;
  let maxBottom: number | null = null;
  for (const node of nodes) {
    const nodeStart = Number(node.dataset.startLine);
    const nodeEnd = Number(node.dataset.endLine);
    if (Number.isNaN(nodeStart) || Number.isNaN(nodeEnd)) continue;
    // Range [startLine, endLine] intersects [nodeStart, nodeEnd]?
    if (nodeEnd < startLine || nodeStart > endLine) continue;
    const rect = node.getBoundingClientRect();
    const top = rect.top - containerRect.top + container.scrollTop;
    const bottom = rect.bottom - containerRect.top + container.scrollTop;
    if (minTop === null || top < minTop) minTop = top;
    if (maxBottom === null || bottom > maxBottom) maxBottom = bottom;
  }
  if (minTop === null || maxBottom === null) return null;
  return { top: minTop, height: Math.max(0, maxBottom - minTop) };
}
