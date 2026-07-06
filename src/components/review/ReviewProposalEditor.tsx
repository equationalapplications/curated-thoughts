import type { RefObject } from "react";
import type { ProposalDetail } from "../../lib/tauri";
import { formatProposalPreview } from "../../lib/proposalPreview";

interface Props {
  detail: ProposalDetail | null | undefined;
  containerRef?: RefObject<HTMLDivElement | null>;
}

export function ReviewProposalEditor({ detail, containerRef }: Props) {
  if (detail === undefined) {
    return <p className="review-hint">Loading proposal…</p>;
  }
  if (detail === null) {
    return <p className="review-hint">Proposal details unavailable.</p>;
  }

  const preview = formatProposalPreview(detail);
  const variant = detail.kind === "new_entity" ? "new" : "update";

  return (
    <div
      className="review-proposal-editor"
      data-variant={variant}
      data-testid="review-proposal-editor"
      ref={containerRef}
      tabIndex={-1}
    >
      <pre className="review-proposal-preview">{preview}</pre>
    </div>
  );
}
