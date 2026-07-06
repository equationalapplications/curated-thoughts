import { describe, it, expect } from "vitest";
import { isWikiDocPath } from "../lib/paths";

describe("isWikiDocPath", () => {
  const root = "/Users/test/Curated-Thoughts";

  it("treats relative paths under top-level wiki/ as wiki", () => {
    expect(isWikiDocPath("wiki/Page.md", root)).toBe(true);
  });

  it("treats relative documents/ paths as non-wiki", () => {
    expect(isWikiDocPath("documents/notes.md", root)).toBe(false);
  });

  it("does not treat documents/wiki/... as wiki", () => {
    expect(isWikiDocPath("documents/wiki/trap.md", root)).toBe(false);
    expect(isWikiDocPath(`${root}/documents/wiki/trap.md`, root)).toBe(false);
  });

  it("treats absolute paths under <root>/wiki/ as wiki", () => {
    expect(isWikiDocPath(`${root}/wiki/Page.md`, root)).toBe(true);
  });

  it("handles Windows-style roots case-insensitively", () => {
    expect(isWikiDocPath("C:/Vault/wiki/Page.md", "c:/vault")).toBe(true);
  });

  it("returns false for null/empty", () => {
    expect(isWikiDocPath(null, root)).toBe(false);
    expect(isWikiDocPath("", root)).toBe(false);
  });
});
