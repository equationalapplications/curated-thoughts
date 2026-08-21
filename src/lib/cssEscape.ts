/**
 * Escape an arbitrary string so it can be safely interpolated inside a
 * CSS attribute selector (e.g. `[data-id="..."]`). Prefers the native
 * `CSS.escape` when available; falls back to a minimal escape for the
 * three characters that are special inside an attribute selector value
 * — `"`, `\`, and `]` — so callers don't have to think about the cases.
 *
 * Used by the chunk-overlay machinery to look up BlockNote blocks by
 * `data-id`, where the id is a generated string that may contain
 * characters that would otherwise break the selector.
 */
export function escapeSelector(s: string): string {
  if (typeof CSS !== "undefined" && typeof CSS.escape === "function") {
    return CSS.escape(s);
  }
  return s.replace(/(["\\\]])/g, "\\$1");
}
