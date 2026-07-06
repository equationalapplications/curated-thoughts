import { describe, expect, it } from "vitest";
import {
  computeChurnRatio,
  computeProposalDiff,
} from "../lib/reviewDiff";
import {
  HIGH_CHURN_NEW,
  HIGH_CHURN_OLD,
  MODERATE_REWRITE_NEW,
  MODERATE_REWRITE_OLD,
  PARAGRAPH_REWRITE_NEW,
  PARAGRAPH_REWRITE_NEW_FACT,
  PARAGRAPH_REWRITE_OLD,
} from "./fixtures/paragraph-rewrite";

describe("computeChurnRatio", () => {
  it("reports high churn for the paragraph-rewrite fixture", () => {
    const ratio = computeChurnRatio(
      PARAGRAPH_REWRITE_OLD,
      PARAGRAPH_REWRITE_NEW,
    );
    expect(ratio).toBeGreaterThan(0.7);
  });

  it("reports low churn for identical text", () => {
    expect(computeChurnRatio("hello world", "hello world")).toBe(0);
  });
});

describe("computeProposalDiff", () => {
  it("uses inline mode for moderate rewrites with a visible fact change", () => {
    const result = computeProposalDiff(
      MODERATE_REWRITE_OLD,
      MODERATE_REWRITE_NEW,
    );

    expect(result.mode).toBe("inline");
    expect(result.churnRatio).toBeLessThanOrEqual(0.7);

    const addedText = result.hunks
      .filter((h) => h.added)
      .map((h) => h.value)
      .join("");
    expect(addedText).toContain(PARAGRAPH_REWRITE_NEW_FACT);
  });

  it("falls back to side-by-side for high-churn paragraph rewrite", () => {
    const result = computeProposalDiff(
      PARAGRAPH_REWRITE_OLD,
      PARAGRAPH_REWRITE_NEW,
    );

    expect(result.mode).toBe("side-by-side");
    expect(result.churnRatio).toBeGreaterThan(0.7);
    expect(result.oldText).toBe(PARAGRAPH_REWRITE_OLD);
    expect(result.newText).toBe(PARAGRAPH_REWRITE_NEW);
  });

  it("falls back to side-by-side for unrelated text", () => {
    const result = computeProposalDiff(HIGH_CHURN_OLD, HIGH_CHURN_NEW);

    expect(result.mode).toBe("side-by-side");
    expect(result.hunks).toHaveLength(0);
  });

  it("keeps inline hunks sparse — not one giant add and one giant remove", () => {
    const result = computeProposalDiff(
      MODERATE_REWRITE_OLD,
      MODERATE_REWRITE_NEW,
    );

    expect(result.mode).toBe("inline");

    const removedHunks = result.hunks.filter((h) => h.removed);
    const addedHunks = result.hunks.filter((h) => h.added);
    const unchangedHunks = result.hunks.filter(
      (h) => !h.added && !h.removed,
    );

    expect(unchangedHunks.length).toBeGreaterThan(0);
    expect(
      Math.max(
        ...removedHunks.map((h) => h.value.length),
        0,
      ),
    ).toBeLessThan(MODERATE_REWRITE_OLD.length * 0.5);
    expect(
      Math.max(...addedHunks.map((h) => h.value.length), 0),
    ).toBeLessThan(MODERATE_REWRITE_NEW.length * 0.5);
  });
});
