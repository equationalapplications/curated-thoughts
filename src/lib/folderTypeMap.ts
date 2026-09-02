/**
 * Optional `ingest.folder_type_map` resolution (spec §2.4).
 *
 * Map iteration order is never load-bearing: JSON object key order is not
 * guaranteed, and this config round-trips through `preserved_keys`
 * (`config/mod.rs:70`) as raw serde_json::Value, whose ordering depends on
 * build features. Globs are sorted into a total order first, so two globs can
 * never tie and the selected type is identical on every platform and parser.
 */

/** Non-wildcard path segments, then total literal (non-metacharacter) length. */
function specificity(glob: string): [number, number] {
  const segments = glob.split('/').filter((s) => s.length > 0 && !s.includes('*') && !s.includes('?'));
  const literalLength = glob.replace(/[*?[\]]/g, '').length;
  return [segments.length, literalLength];
}

/** Total order: descending specificity, then ascending lexicographic. */
export function orderGlobs(map: Record<string, string>): string[] {
  return Object.keys(map).sort((a, b) => {
    const [aSeg, aLen] = specificity(a);
    const [bSeg, bLen] = specificity(b);
    if (aSeg !== bSeg) return bSeg - aSeg;
    // Within the same fixed-segment count, the more **specific** glob — the
    // one with the longer total literal (less wildcard) — wins. Sorting
    // descending on literal length picks it first.
    if (aLen !== bLen) return bLen - aLen;
    return a < b ? -1 : a > b ? 1 : 0;
  });
}

/** Every character that is meaningful to a RegExp and must survive as a literal. */
const REGEX_METACHARS = /[.*+?^${}()|[\]\\]/g;

/**
 * Anchored glob match supporting `**` (across segments), `*` (within a
 * segment) and `?` (one non-separator character).
 *
 * Translated by a single left-to-right scan rather than by chained
 * `String.replace` calls. Chaining is not safe here: escaping a fixed
 * metacharacter list and then rewriting `*` leaves `?` behind as a live
 * quantifier (so `notes?/**` would match `note/x.md`), and staging `**`
 * through a placeholder character corrupts any glob that legitimately
 * contains that character (a space, for `my docs/**`). Scanning once means
 * every character is classified exactly as either a wildcard or a literal.
 */
function globMatches(glob: string, path: string): boolean {
  let pattern = '';
  for (let i = 0; i < glob.length; i += 1) {
    const char = glob[i];
    if (char === '*') {
      if (glob[i + 1] === '*') {
        pattern += '.*';
        i += 1;
      } else {
        pattern += '[^/]*';
      }
    } else if (char === '?') {
      pattern += '[^/]';
    } else {
      pattern += char.replace(REGEX_METACHARS, '\\$&');
    }
  }
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
