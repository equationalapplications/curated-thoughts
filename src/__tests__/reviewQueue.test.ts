import { describe, expect, it } from "vitest";
import {
  adjacentQueueId,
  nextQueueSelectionId,
  sortReviewQueue,
} from "../lib/reviewQueue";

const QUEUE = [
  { id: 3, path: "c" },
  { id: 1, path: "a" },
  { id: 2, path: "b" },
];

describe("sortReviewQueue", () => {
  it("orders by id ascending", () => {
    expect(sortReviewQueue(QUEUE).map((p) => p.id)).toEqual([1, 2, 3]);
  });
});

describe("nextQueueSelectionId", () => {
  it("selects the next item after the current one is removed", () => {
    expect(nextQueueSelectionId(QUEUE, 1)).toBe(2);
    expect(nextQueueSelectionId(QUEUE, 2)).toBe(3);
  });

  it("selects the previous item when the last one is removed", () => {
    expect(nextQueueSelectionId(QUEUE, 3)).toBe(2);
  });
});

describe("adjacentQueueId", () => {
  it("moves next and prev in sorted order", () => {
    const sorted = sortReviewQueue(QUEUE);
    expect(adjacentQueueId(sorted, 1, "next")).toBe(2);
    expect(adjacentQueueId(sorted, 2, "prev")).toBe(1);
    expect(adjacentQueueId(sorted, 1, "prev")).toBe(1);
  });
});
