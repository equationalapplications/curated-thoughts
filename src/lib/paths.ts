function isAbsolutePath(norm: string): boolean {
  return norm.startsWith("/") || /^[A-Za-z]:\//.test(norm);
}

/** Strip configured vault root from an absolute path; otherwise null. */
function vaultRelative(norm: string, vaultRoot: string): string | null {
  const n = norm.replace(/\\/g, "/");
  const root = vaultRoot.replace(/\\/g, "/").replace(/\/+$/, "");
  if (!root) return null;

  // Windows paths are case-insensitive; Unix paths are case-sensitive.
  const caseInsensitive = /^[A-Za-z]:\//.test(root);
  const lhs = n.slice(0, root.length);
  const rhs = root;
  const matchesPrefix = caseInsensitive
    ? lhs.toLowerCase() === rhs.toLowerCase()
    : lhs === rhs;

  if (n.length >= root.length && matchesPrefix) {
    if (n.length === root.length) return "";
    const sep = n[root.length];
    if (sep === "/" || sep === "\\") {
      return n.slice(root.length + 1);
    }
  }
  return null;
}

/**
 * Wiki docs live under the vault's top-level `wiki/` directory only.
 * Avoid `includes("/wiki/")` so `documents/wiki/...` is not treated as wiki.
 */
export function isWikiDocPath(p: string | null | undefined, vaultRoot: string): boolean {
  if (!p) return false;
  const norm = p.replace(/\\/g, "/");
  if (!isAbsolutePath(norm)) {
    const first = norm.split("/").filter(Boolean)[0];
    return first === "wiki";
  }
  const rel = vaultRelative(norm, vaultRoot);
  if (rel === null || rel === "") return false;
  const first = rel.split("/").filter(Boolean)[0];
  return first === "wiki";
}
