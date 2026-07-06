import { diffWordsWithSpace } from "diff";

/** Fraction of words changed before falling back to side-by-side view. */
export const CHURN_THRESHOLD = 0.7;

export type DiffHunk = {
  value: string;
  added?: boolean;
  removed?: boolean;
};

export type ProposalDiffResult = {
  mode: "inline" | "side-by-side";
  hunks: DiffHunk[];
  churnRatio: number;
  oldText: string;
  newText: string;
};

function countWords(text: string): number {
  return text.trim().split(/\s+/).filter(Boolean).length;
}

export function computeChurnRatio(oldText: string, newText: string): number {
  const changes = diffWordsWithSpace(oldText, newText);
  let changedWords = 0;

  for (const change of changes) {
    if (change.added || change.removed) {
      changedWords += countWords(change.value);
    }
  }

  const totalWords = Math.max(countWords(oldText), countWords(newText));
  if (totalWords === 0) return 0;

  return Math.min(1, changedWords / totalWords);
}

export function computeProposalDiff(
  oldText: string,
  newText: string,
): ProposalDiffResult {
  const churnRatio = computeChurnRatio(oldText, newText);
  const mode =
    churnRatio > CHURN_THRESHOLD ? "side-by-side" : "inline";

  if (mode === "side-by-side") {
    return { mode, hunks: [], churnRatio, oldText, newText };
  }

  const hunks: DiffHunk[] = diffWordsWithSpace(oldText, newText).map(
    (change) => ({
      value: change.value,
      ...(change.added ? { added: true } : {}),
      ...(change.removed ? { removed: true } : {}),
    }),
  );

  return { mode, hunks, churnRatio, oldText, newText };
}
