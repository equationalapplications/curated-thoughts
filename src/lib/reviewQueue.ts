/** Oldest-first queue order (matches editorial desk default). */
export function sortReviewQueue<T extends { id: number }>(queue: T[]): T[] {
  return [...queue].sort((a, b) => a.id - b.id);
}

/** After removing `currentId`, pick the next focused item in sorted order. */
export function nextQueueSelectionId<T extends { id: number }>(
  queue: T[],
  currentId: number,
): number | null {
  const sorted = sortReviewQueue(queue);
  const index = sorted.findIndex((page) => page.id === currentId);
  const remaining = sorted.filter((page) => page.id !== currentId);
  if (remaining.length === 0) return null;
  if (index === -1) return remaining[0]?.id ?? null;
  if (index < remaining.length) return remaining[index].id;
  return remaining[remaining.length - 1].id;
}

export function adjacentQueueId<T extends { id: number }>(
  queue: T[],
  currentId: number,
  direction: "next" | "prev",
): number | null {
  const sorted = sortReviewQueue(queue);
  const index = sorted.findIndex((page) => page.id === currentId);
  if (index === -1) return sorted[0]?.id ?? null;

  const nextIndex = direction === "next" ? index + 1 : index - 1;
  if (nextIndex < 0 || nextIndex >= sorted.length) return currentId;
  return sorted[nextIndex].id;
}
