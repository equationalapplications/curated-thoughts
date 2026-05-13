export function entityIdForPath(
  vaultRelativePath: string,
  workspaceId: string
): {
  entityId: string;
  sourceType: 'immutable_document' | 'user_confirmed' | 'librarian_inferred';
} {
  if (vaultRelativePath.startsWith('documents/')) {
    return { entityId: 'tier_fact', sourceType: 'immutable_document' };
  }
  if (vaultRelativePath.startsWith('wiki/')) {
    return { entityId: 'tier_wisdom', sourceType: 'user_confirmed' };
  }
  return { entityId: workspaceId, sourceType: 'librarian_inferred' };
}
