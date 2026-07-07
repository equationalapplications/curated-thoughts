export interface WikilinkSegment {
  type: "text" | "link";
  value: string;
}

const WIKILINK_RE = /\[\[([^[\]]+)\]\]/g;

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

export function WikilinkText({ text, onNavigate }: Props) {
  return (
    <span className="wikilink-text">
      {parseWikilinks(text).map((segment, index) =>
        segment.type === "link" ? (
          <button
            key={index}
            type="button"
            className="wikilink-chip"
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
