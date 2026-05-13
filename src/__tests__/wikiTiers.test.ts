import { describe, it, expect } from 'vitest';
import { entityIdForPath } from '../lib/wikiTiers';

describe('entityIdForPath', () => {
  const workspaceId = 'tier_working::a3f9b2c1d4e5f607';

  it('returns tier_fact + immutable_document for documents/ prefix', () => {
    expect(entityIdForPath('documents/api-ref.md', workspaceId)).toEqual({
      entityId: 'tier_fact',
      sourceType: 'immutable_document',
    });
  });

  it('returns tier_fact for files in documents subdirectory', () => {
    expect(entityIdForPath('documents/specs/v2/design.md', workspaceId)).toEqual({
      entityId: 'tier_fact',
      sourceType: 'immutable_document',
    });
  });

  it('returns tier_wisdom + user_confirmed for wiki/ prefix', () => {
    expect(entityIdForPath('wiki/auth-patterns.md', workspaceId)).toEqual({
      entityId: 'tier_wisdom',
      sourceType: 'user_confirmed',
    });
  });

  it('returns workspaceId + librarian_inferred for src/ path', () => {
    expect(entityIdForPath('src/db/init.rs', workspaceId)).toEqual({
      entityId: workspaceId,
      sourceType: 'librarian_inferred',
    });
  });

  it('returns workspaceId + librarian_inferred for root-level file', () => {
    expect(entityIdForPath('README.md', workspaceId)).toEqual({
      entityId: workspaceId,
      sourceType: 'librarian_inferred',
    });
  });

  it('does not match documentsfoo/ as documents/', () => {
    expect(entityIdForPath('documentsfoo/bar.md', workspaceId).entityId).toBe(workspaceId);
  });
});
