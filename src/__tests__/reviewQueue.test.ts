import { describe, expect, it } from "vitest";
import {
  adjacentQueueId,
  nextQueueSelectionId,
  sortReviewQueue,
} from "../lib/reviewQueue";

const QUEUE = [
  { id: "prop_c", created_at: 300, path: "c" },
  { id: "prop_a", created_at: 100, path: "a" },
  { id: "prop_b", created_at: 200, path: "b" },
];

describe("sortReviewQueue", () => {
  it("orders by created_at ascending", () => {
    expect(sortReviewQueue(QUEUE).map((p) => p.id)).toEqual([
      "prop_a",
      "prop_b",
      "prop_c",
    ]);
  });
});

describe("nextQueueSelectionId", () => {
  it("selects the next item after the current one is removed", () => {
    expect(nextQueueSelectionId(QUEUE, "prop_a")).toBe("prop_b");
    expect(nextQueueSelectionId(QUEUE, "prop_b")).toBe("prop_c");
  });

  it("selects the previous item when the last one is removed", () => {
    expect(nextQueueSelectionId(QUEUE, "prop_c")).toBe("prop_b");
  });
});

describe("adjacentQueueId", () => {
  it("moves next and prev in sorted order", () => {
    const sorted = sortReviewQueue(QUEUE);
    expect(adjacentQueueId(sorted, "prop_a", "next")).toBe("prop_b");
    expect(adjacentQueueId(sorted, "prop_b", "prev")).toBe("prop_a");
    expect(adjacentQueueId(sorted, "prop_a", "prev")).toBe("prop_a");
  });
});
