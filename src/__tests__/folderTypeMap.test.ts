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
