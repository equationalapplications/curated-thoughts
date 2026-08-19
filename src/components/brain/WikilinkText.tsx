import { useEffect, useState } from "react";
import { listEntities, type EntitySummary } from "../../lib/tauri";

export interface WikilinkSegment {
  type: "text" | "link";
  value: string;
}

const WIKILINK_RE = /\[\[([^[\]]+)\]\]/g;

const RESOLVED_CLASS = "wikilink-chip--resolved";
const UNRESOLVED_CLASS = "wikilink-chip--unresolved";

export function parseWikilinks(text: string): WikilinkSegment[] {
  const segments: WikilinkSegment[] = [];
  let last = 0;
  let match: RegExpExecArray | null;
  WIKILINK_RE.lastIndex = 0;
  while ((match = WIKILINK_RE.exec(text)) !== null) {
    if (match.index > last) {
      segments.push({ type: "text", value: text.slice(last, match.index) });
    }
    segments.push({ type: "link", value: match[1] });
    last = match.index + match[0].length;
  }
  if (last < text.length) {
    segments.push({ type: "text", value: text.slice(last) });
  }
  return segments;
}

interface Props {
  text: string;
  onNavigate: (entityName: string) => void;
}

/**
 * Shared, module-level cache so every `<WikilinkText>` instance subscribes to
 * the same single `listEntities` fetch — a summary with N wikilink chips
 * issues one round-trip, not N. Subscribers re-render when the cache updates.
 */
type ResolverState =
  | { status: "loading"; promise: Promise<void>; version: number; fetchStarted: boolean }
  | { status: "ready"; names: Set<string>; entities: EntitySummary[]; version: number };

let resolverState: ResolverState = {
  status: "loading",
  promise: Promise.resolve(),
  version: 0,
  // Tracks whether a fetch has already been launched. Without this flag,
  // every mount while the cache is in "loading" would issue its own
  // `listEntities` round-trip — defeating the dedup the comment promises.
  fetchStarted: false,
};
const resolverListeners = new Set<() => void>();

function setReadyState(entities: EntitySummary[], version: number) {
  const safeEntities = entities ?? [];
  const names = new Set(safeEntities.map((e) => e.name.toLowerCase()));
  resolverState = { status: "ready", names, entities: safeEntities, version };
  resolverListeners.forEach((fn) => fn());
}

function ensureResolver(): Promise<void> {
  if (resolverState.status === "ready") return Promise.resolve();
  if (resolverState.fetchStarted) return resolverState.promise;
  const existing = resolverState;
  // Wrap in Promise.resolve so a non-thenable (e.g. undefined from a test
  // mock of `invoke`) becomes a fulfilled promise instead of crashing here.
  const promise = Promise.resolve(listEntities("name_asc"))
    .then((entities) => {
      setReadyState(entities ?? [], existing.version + 1);
    })
    .catch(() => {
      // Reset so future mounts retry; otherwise we'd be stuck in "loading"
      // with no fetch in flight and every `[[Name]]` would render unresolved.
      resolverState = {
        status: "loading",
        promise: Promise.resolve(),
        version: existing.version + 1,
        fetchStarted: false,
      };
      resolverListeners.forEach((fn) => fn());
    });
  resolverState = {
    status: "loading",
    promise,
    version: existing.version + 1,
    fetchStarted: true,
  };
  return promise;
}

/**
 * Force the resolver cache to re-fetch. Call this after any entity list
 * mutation (create, import, delete, merge) so newly created entities render
 * as resolved in subsequent `[[Name]]` chips. Returns the fetch promise so
 * callers can await the refresh when ordering matters.
 */
export function refreshWikilinkResolver(): Promise<void> {
  const existing = resolverState;
  resolverState = {
    status: "loading",
    promise: Promise.resolve(),
    version: existing.version + 1,
    fetchStarted: false,
  };
  resolverListeners.forEach((fn) => fn());
  return ensureResolver();
}

/**
 * Returns the currently cached entity list (or `[]` while loading). Used by
 * the `[[Entity]]` autocomplete to avoid an IPC round-trip per keystroke.
 */
export function getWikilinkResolverEntities(): EntitySummary[] {
  return resolverState.status === "ready" ? resolverState.entities : [];
}

/**
 * Resolves entity names (case-insensitive) against the live entity list.
 * Returns a Set of lowercase names that have a matching entity.
 */
function useResolvedEntityNames(): Set<string> {
  const [, setVersion] = useState(resolverState.version);

  useEffect(() => {
    ensureResolver();
    const listener = () => setVersion((v) => v + 1);
    resolverListeners.add(listener);
    return () => {
      resolverListeners.delete(listener);
    };
  }, []);

  return resolverState.status === "ready" ? resolverState.names : new Set();
}

export function WikilinkText({ text, onNavigate }: Props) {
  const resolvedNames = useResolvedEntityNames();

  return (
    <span className="wikilink-text">
      {parseWikilinks(text).map((segment, index) =>
        segment.type === "link" ? (
          <button
            key={index}
            type="button"
            className={`wikilink-chip ${
              resolvedNames.has(segment.value.toLowerCase())
                ? RESOLVED_CLASS
                : UNRESOLVED_CLASS
            }`}
            onClick={() => onNavigate(segment.value)}
          >
            {segment.value}
          </button>
        ) : (
          <span key={index}>{segment.value}</span>
        ),
      )}
    </span>
  );
}
