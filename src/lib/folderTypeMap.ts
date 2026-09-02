/**
 * Optional `ingest.folder_type_map` resolution (spec §2.4).
 *
 * Map iteration order is never load-bearing: JSON object key order is not
 * guaranteed, and this config round-trips through `preserved_keys`
 * (`config/mod.rs:70`) as raw serde_json::Value, whose ordering depends on
 * build features. Globs are sorted into a total order first, so two globs can
 * never tie and the selected type is identical on every platform and parser.
 */

/** Non-wildcard path segments, then negative total literal length. */
function specificity(glob: string): [number, number] {
  const segments = glob.split('/').filter((s) => s.length > 0 && !s.includes('*') && !s.includes('?'));
  const literalLength = glob.replace(/[*?[\]]/g, '').length;
  return [segments.length, -literalLength];
}

/** Total order: descending specificity, then ascending lexicographic. */
export function orderGlobs(map: Record<string, string>): string[] {
  return Object.keys(map).sort((a, b) => {
    const [aSeg, aLen] = specificity(a);
    const [bSeg, bLen] = specificity(b);
    if (aSeg !== bSeg) return bSeg - aSeg;
    if (aLen !== bLen) return bLen - aLen;
    return a < b ? -1 : a > b ? 1 : 0;
  });
}

/** Anchored glob match supporting `*` (within a segment) and `**` (across segments). */
function globMatches(glob: string, path: string): boolean {
  const escaped = glob.replace(/[.+^${}()|[\]\\]/g, '\\$&');
  const pattern = escaped
    .replace(/\*\*/g, ' ')
    .replace(/\*/g, '[^/]*')
    .replace(/ /g, '.*');
  return new RegExp(`^${pattern}$`).test(path);
}

/**
 * The manifest node type for a path, or null when nothing matches.
 * Never a validation gate: an unmatched document ingests unclassified.
 */
export function resolveFolderType(
  map: Record<string, string>,
  vaultRelativePath: string,
): string | null {
  for (const glob of orderGlobs(map)) {
    if (globMatches(glob, vaultRelativePath)) return map[glob];
  }
  return null;
}
