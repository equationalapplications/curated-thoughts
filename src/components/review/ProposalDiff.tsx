import { computeProposalDiff } from "../../lib/reviewDiff";

interface Props {
  oldText: string;
  newText: string;
}

export function ProposalDiff({ oldText, newText }: Props) {
  const result = computeProposalDiff(oldText, newText);

  if (result.mode === "side-by-side") {
    return (
      <div
        className="proposal-diff proposal-diff--side-by-side"
        data-mode="side-by-side"
        data-testid="proposal-diff"
      >
        <div className="proposal-diff-pane">
          <h4 className="proposal-diff-label">Current</h4>
          <pre className="proposal-diff-text">{result.oldText}</pre>
        </div>
        <div className="proposal-diff-pane">
          <h4 className="proposal-diff-label">Proposed</h4>
          <pre className="proposal-diff-text">{result.newText}</pre>
        </div>
      </div>
    );
  }

  return (
    <div
      className="proposal-diff proposal-diff--inline"
      data-mode="inline"
      data-testid="proposal-diff"
    >
      {result.hunks.map((hunk, index) => {
        let className: string | undefined;
        if (hunk.added) className = "proposal-diff-added";
        else if (hunk.removed) className = "proposal-diff-removed";

        return (
          <span key={index} className={className}>
            {hunk.value}
          </span>
        );
      })}
    </div>
  );
}
