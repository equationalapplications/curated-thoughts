import { useEffect, useRef, useState } from "react";
import { resolveChunkOverlay } from "../lib/tauri";

export type ChunkOverlayStatus = "idle" | "source-moved" | "visible";

export interface ChunkOverlayRect {
  top: number;
  height: number;
}

export type BlockLineRange = readonly [startLine: number, endLine: number];

/** Map of `BlockNote` block id → (startLine, endLine) in the original
 * markdown source. The EditorPane builds this once per doc-load by
 * walking the markdown source alongside the parsed BlockNote blocks
 * and writing it onto each block's DOM element as `data-start-line` /
 * `data-end-line` attributes.
 *
 * The hook reads it back via the cached block-id list — no DOM walks
 * needed for the rect on scroll. */
export type BlockLineMap = Map<string, BlockLineRange>;

export interface UseChunkOverlayResult {
  status: ChunkOverlayStatus;
  overlay: ChunkOverlayRect | null;
  /** Raw line range the IPC resolved; useful for tests + callers. */
  range: { startLine: number; endLine: number } | null;
}

const MAX_RETRY_FRAMES = 5;

/**
 * Resolves a chunk-hash anchor to a line range, then looks up the
 * matching BlockNote block IDs in `lineMap` and computes a pixel
 * rect from those DOM elements.
 *
 * Returns idle for empty hash (no-op), source-moved when the IPC
 * returns null or rejects, and source-moved when no blocks cover
 * the resolved range OR the matching DOM nodes never paint (after
 * up to MAX_RETRY_FRAMES rAF retries). When everything succeeds,
 * status is 'visible' and `overlay` carries top/height.
 *
 * The `container` is the editor pane root (passed as state, not a
 * RefObject — using state means the effect re-runs once the pane
 * mounts, so the rect computation isn't gated on a stale ref).
 */
export function useChunkOverlay(
  path: string | null,
  hash: string | null,
  container: HTMLElement | null,
  lineMap: BlockLineMap,
): UseChunkOverlayResult {
  const [status, setStatus] = useState<ChunkOverlayStatus>("idle");
  const [overlay, setOverlay] = useState<ChunkOverlayRect | null>(null);
  const [range, setRange] = useState<{ startLine: number; endLine: number } | null>(null);
  const cancelledRef = useRef(false);
  /** Cached block ids matching the current `range`. Populated by the
   * rect-computation effect; read by the scroll listener (so we don't
   * re-query `[data-id]` on every scroll event). */
  const cachedBlockIdsRef = useRef<string[]>([]);
  /** Latest lineMap, accessed by the scroll listener without forcing
   * the listener to re-subscribe on every EditorPane re-render. */
  const lineMapRef = useRef<BlockLineMap>(lineMap);
  lineMapRef.current = lineMap;

  // Effect A: IPC resolution — depends only on (path, hash).
  useEffect(() => {
    cancelledRef.current = false;
    if (!hash) {
      setStatus("idle");
      setOverlay(null);
      setRange(null);
      cachedBlockIdsRef.current = [];
      return;
    }
    setStatus("idle");
    setOverlay(null);
    setRange(null);
    cachedBlockIdsRef.current = [];
    resolveChunkOverlay(path ?? "", hash)
      .then((res) => {
        if (cancelledRef.current) return;
        if (!res) {
          setStatus("source-moved");
          return;
        }
        setRange(res);
        // Don't set status here — Effect B handles it once the rect
        // is computed (or falls back to source-moved).
      })
      .catch(() => {
        if (cancelledRef.current) return;
        setStatus("source-moved");
      });
    return () => {
      cancelledRef.current = true;
    };
  }, [path, hash]);

  // Effect B: rect computation — re-runs whenever the resolved range,
  // the line-to-block map, or the container element changes. Retries
  // up to MAX_RETRY_FRAMES animation frames before giving up and
  // surfacing the source-moved notice (handles slow paint: BlockNote
  // may take a frame or two to mount after `replaceBlocks`).
  useEffect(() => {
    if (!range) return;
    let cancelled = false;
    let retryCount = 0;
    let scrolledRef = false;

    const attempt = () => {
      if (cancelled) return;
      const ids = findOverlappingBlocks(range, lineMapRef.current);
      cachedBlockIdsRef.current = ids;
      const rect = computeRectFromBlockIds(ids, container);
      if (rect) {
        setOverlay(rect);
        setStatus("visible");
        // Scroll the first matching block into view once. Subsequent
        // status flips (e.g., lineMap updates) reset the flag.
        if (!scrolledRef && ids.length > 0 && container) {
          scrolledRef = true;
          const firstId = ids[0];
          const firstEl = container.querySelector<HTMLElement>(
            `[data-id="${escapeSelector(firstId)}"]`,
          );
          if (firstEl && typeof firstEl.scrollIntoView === "function") {
            firstEl.scrollIntoView({ block: "nearest", behavior: "auto" });
          }
        }
        return;
      }
      if (retryCount < MAX_RETRY_FRAMES) {
        retryCount++;
        requestAnimationFrame(attempt);
        return;
      }
      // Out of retries — no matching DOM nodes. Per spec, this is the
      // "line-to-block mapping failure" case: surface source-moved so
      // the user at least sees the badge instead of silent nothing.
      setStatus("source-moved");
      setOverlay(null);
    };

    // First attempt on next frame (so BlockNote has a chance to paint).
    requestAnimationFrame(attempt);

    return () => {
      cancelled = true;
    };
  }, [range, lineMap, container]);

  // Effect C: scroll tracking — re-reads the cached block ids' rect
  // on scroll. Uses the cached ids (no fresh DOM walk); only the
  // getBoundingClientRect call, which is necessary for accurate
  // top/height relative to the scrolling container.
  useEffect(() => {
    if (status !== "visible") return;
    const root = container;
    if (!root) return;
    function onScroll() {
      const ids = cachedBlockIdsRef.current;
      if (ids.length === 0) return;
      const rect = computeRectFromBlockIds(ids, root);
      if (rect) setOverlay(rect);
    }
    root.addEventListener("scroll", onScroll, { passive: true });
    return () => root.removeEventListener("scroll", onScroll);
  }, [status, container]);

  return { status, overlay, range };
}

/**
 * Find every block whose (startLine, endLine) intersects the resolved
 * range. Range intersection is inclusive at both ends; a block whose
 * `startLine === range.endLine + 1` is NOT considered overlapping.
 */
function findOverlappingBlocks(
  range: { startLine: number; endLine: number },
  lineMap: BlockLineMap,
): string[] {
  const ids: string[] = [];
  for (const [id, [bs, be]] of lineMap) {
    if (be < range.startLine || bs > range.endLine) continue;
    ids.push(id);
  }
  return ids;
}

/**
 * Compute the union top/height of the supplied block ids within the
 * container's coordinate system. Returns `null` if any block id is
 * missing from the DOM (caller retries on next frame).
 */
function computeRectFromBlockIds(
  blockIds: string[],
  container: HTMLElement | null,
): ChunkOverlayRect | null {
  if (!container || blockIds.length === 0) return null;
  const containerRect = container.getBoundingClientRect();
  let minTop: number | null = null;
  let maxBottom: number | null = null;
  for (const id of blockIds) {
    const node = container.querySelector<HTMLElement>(
      `[data-id="${escapeSelector(id)}"]`,
    );
    if (!node) return null;
    const rect = node.getBoundingClientRect();
    const top = rect.top - containerRect.top + container.scrollTop;
    const bottom = rect.bottom - containerRect.top + container.scrollTop;
    if (minTop === null || top < minTop) minTop = top;
    if (maxBottom === null || bottom > maxBottom) maxBottom = bottom;
  }
  if (minTop === null || maxBottom === null) return null;
  return { top: minTop, height: Math.max(0, maxBottom - minTop) };
}

function escapeSelector(s: string): string {
  if (typeof CSS !== "undefined" && typeof CSS.escape === "function") {
    return CSS.escape(s);
  }
  return s.replace(/(["\\\]])/g, "\\$1");
}
