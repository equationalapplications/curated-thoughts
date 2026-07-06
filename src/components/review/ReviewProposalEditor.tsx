import type { RefObject } from "react";
import type { ProposalDetail } from "../../lib/tauri";
import { formatProposalPreview } from "../../lib/proposalPreview";

interface Props {
  detail: ProposalDetail | null;
  containerRef?: RefObject<HTMLDivElement | null>;
}

export function ReviewProposalEditor({ detail, containerRef }: Props) {
  if (detail === null) {
    return <p className="review-hint">Loading proposal…</p>;
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
