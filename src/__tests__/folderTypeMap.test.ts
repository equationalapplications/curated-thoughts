// src/__tests__/folderTypeMap.test.ts
import { describe, it, expect } from 'vitest';
import { resolveFolderType, orderGlobs } from '../lib/folderTypeMap';

describe('orderGlobs', () => {
  it('orders by descending literal specificity, then ascending lexicographic', () => {
    const map = { 'people/**': 'Person', 'people/execs/**': 'Executive', 'a/**': 'A' };
    expect(orderGlobs(map)).toEqual(['people/execs/**', 'a/**', 'people/**']);
  });

  it('is independent of key insertion order', () => {
    const a = { 'people/**': 'Person', 'people/execs/**': 'Executive' };
    const b = { 'people/execs/**': 'Executive', 'people/**': 'Person' };
    expect(orderGlobs(a)).toEqual(orderGlobs(b));
  });
});

describe('resolveFolderType', () => {
  it('returns the most specific matching type', () => {
    const map = { 'people/**': 'Person', 'people/execs/**': 'Executive' };
    expect(resolveFolderType(map, 'people/execs/ada.md')).toBe('Executive');
    expect(resolveFolderType(map, 'people/ada.md')).toBe('Person');
  });

  it('classifies identically regardless of key order', () => {
    const a = { 'people/**': 'Person', 'people/execs/**': 'Executive' };
    const b = { 'people/execs/**': 'Executive', 'people/**': 'Person' };
    expect(resolveFolderType(a, 'people/execs/ada.md')).toBe(resolveFolderType(b, 'people/execs/ada.md'));
  });

  it('returns null for an unmatched path — never a validation gate', () => {
    expect(resolveFolderType({ 'people/**': 'Person' }, 'products/x.md')).toBeNull();
  });

  it('returns null for an empty map', () => {
    expect(resolveFolderType({}, 'people/ada.md')).toBeNull();
  });
});

describe('globMatches edge cases', () => {
  it('treats ? as a single non-separator character, not a regex quantifier', () => {
    const map = { 'notes?/**': 'Note' };
    // The `?` must consume exactly one character...
    expect(resolveFolderType(map, 'notes1/x.md')).toBe('Note');
    // ...and must NOT behave as "zero or one of the preceding character",
    // which would have let `notes?/` match the literal path `note/`.
    expect(resolveFolderType(map, 'note/x.md')).toBeNull();
    // `?` never crosses a path separator.
    expect(resolveFolderType(map, 'notes/x.md')).toBeNull();
  });

  it('keeps a literal space in a glob literal', () => {
    // A placeholder-based `**` rewrite corrupted globs containing the
    // placeholder character; a space must stay a space.
    const map = { 'my docs/**': 'Doc' };
    expect(resolveFolderType(map, 'my docs/a.md')).toBe('Doc');
    expect(resolveFolderType(map, 'myXdocs/a.md')).toBeNull();
  });

  it('keeps regex metacharacters literal', () => {
    const map = { 'v1.0/**': 'Release' };
    expect(resolveFolderType(map, 'v1.0/notes.md')).toBe('Release');
    // `.` must not match an arbitrary character.
    expect(resolveFolderType(map, 'v1x0/notes.md')).toBeNull();
  });

  it('* stays within one segment while ** crosses segments', () => {
    expect(resolveFolderType({ 'people/*': 'Person' }, 'people/ada.md')).toBe('Person');
    expect(resolveFolderType({ 'people/*': 'Person' }, 'people/execs/ada.md')).toBeNull();
    expect(resolveFolderType({ 'people/**': 'Person' }, 'people/execs/ada.md')).toBe('Person');
  });

  it('orders two globs with equal segment counts deterministically', () => {
    // `people/*` and `people/**` both have one non-wildcard segment, so the
    // literal-length and lexicographic tie-breaks decide — and must decide the
    // same way regardless of key insertion order.
    const a = { 'people/*': 'Shallow', 'people/**': 'Deep' };
    const b = { 'people/**': 'Deep', 'people/*': 'Shallow' };
    expect(orderGlobs(a)).toEqual(orderGlobs(b));
    expect(resolveFolderType(a, 'people/ada.md')).toBe(resolveFolderType(b, 'people/ada.md'));
  });
});
