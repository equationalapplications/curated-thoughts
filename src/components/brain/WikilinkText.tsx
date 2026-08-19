import { useEffect, useState } from "react";
import { listEntities } from "../../lib/tauri";

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
 * Resolves entity names (case-insensitive) against the live entity list.
 * Returns a Set of lowercase names that have a matching entity.
 */
function useResolvedEntityNames(): Set<string> {
  const [resolved, setResolved] = useState<Set<string>>(new Set());

  useEffect(() => {
    let cancelled = false;
    listEntities("name_asc").then((entities) => {
      if (cancelled) return;
      if (!entities) return;
      setResolved(
        new Set(entities.map((e) => e.name.toLowerCase())),
      );
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return resolved;
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
