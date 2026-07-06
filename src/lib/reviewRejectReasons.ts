const STORAGE_KEY = "ct-review-reject-reasons";

type RejectReasonMap = Record<string, string>;

function readMap(): RejectReasonMap {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return {};
    return parsed as RejectReasonMap;
  } catch {
    return {};
  }
}

function writeMap(map: RejectReasonMap): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(map));
  } catch {
    /* private browsing */
  }
}

export function saveRejectReason(proposalId: string, reason: string): void {
  const map = readMap();
  map[proposalId] = reason;
  writeMap(map);
}

export function getRejectReason(proposalId: string): string | undefined {
  return readMap()[proposalId];
}
