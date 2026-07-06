import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  getProposalDetail,
  resolveProposal,
  type ProposalDetail,
  type ProposalSummary,
} from "../../lib/tauri";
import { useIndexingStatus } from "../../hooks/useIndexingStatus";
import { useReviewKeyboard } from "../../hooks/useReviewKeyboard";
import {
  adjacentQueueId,
  nextQueueSelectionId,
  sortReviewQueue,
} from "../../lib/reviewQueue";
import { allAcceptDecisions, allRejectDecisions } from "../../lib/reviewDecisions";
import { saveRejectReason } from "../../lib/reviewRejectReasons";
import { ReviewQueueList } from "../review/ReviewQueueList";
import { ReviewEvidencePanel } from "../review/ReviewEvidencePanel";
import { ReviewProposalEditor } from "../review/ReviewProposalEditor";

interface Props {
  queue: ProposalSummary[];
  onAction: () => void;
  vaultPath: string;
}

export function ReviewMode({ queue, onAction, vaultPath }: Props) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [checkedIds, setCheckedIds] = useState<Set<string>>(() => new Set());
  const [busy, setBusy] = useState(false);
  const [detail, setDetail] = useState<ProposalDetail | null>(null);
  const editorRef = useRef<HTMLDivElement>(null);
  const { indexed } = useIndexingStatus(vaultPath);

  const sortedQueue = useMemo(() => sortReviewQueue(queue), [queue]);
  const proposal =
    sortedQueue.find((p) => p.id === selectedId) ?? sortedQueue[0] ?? null;

  useEffect(() => {
    setCheckedIds((prev) => {
      const next = new Set<string>();
      for (const id of prev) {
        if (queue.some((p) => p.id === id)) next.add(id);
      }
      return next;
    });
  }, [queue]);

  useEffect(() => {
    setDetail(null);
    if (!proposal) return;
    getProposalDetail(proposal.id)
      .then((loaded) => setDetail(loaded))
      .catch(() => setDetail(null));
  }, [proposal?.id]);

  const handleToggleChecked = useCallback((id: string, checked: boolean) => {
    setCheckedIds((prev) => {
      const next = new Set(prev);
      if (checked) next.add(id);
      else next.delete(id);
      return next;
    });
  }, []);

  const handleSelectNext = useCallback(() => {
    if (!proposal) return;
    const nextId = adjacentQueueId(sortedQueue, proposal.id, "next");
    if (nextId !== null) setSelectedId(nextId);
  }, [proposal, sortedQueue]);

  const handleSelectPrev = useCallback(() => {
    if (!proposal) return;
    const prevId = adjacentQueueId(sortedQueue, proposal.id, "prev");
    if (prevId !== null) setSelectedId(prevId);
  }, [proposal, sortedQueue]);

  const handleFocusEditor = useCallback(() => {
    editorRef.current?.focus();
  }, []);

  const commitProposal = useCallback(
    async (
      proposalId: string,
      loadedDetail: ProposalDetail,
      mode: "accept" | "reject",
      rejectReason?: string,
    ) => {
      const decisions =
        mode === "accept"
          ? allAcceptDecisions(loadedDetail)
          : allRejectDecisions(loadedDetail);
      await resolveProposal(
        proposalId,
        decisions,
        mode === "reject" ? rejectReason : undefined,
      );
    },
    [],
  );

  const handleApprove = useCallback(async () => {
    if (!proposal || !detail || busy) return;
    setBusy(true);
    try {
      await commitProposal(proposal.id, detail, "accept");
      const nextId = nextQueueSelectionId(sortedQueue, proposal.id);
      setSelectedId(nextId);
      setCheckedIds((prev) => {
        const next = new Set(prev);
        next.delete(proposal.id);
        return next;
      });
      onAction();
    } finally {
      setBusy(false);
    }
  }, [proposal, detail, busy, commitProposal, sortedQueue, onAction]);

  const handleReject = useCallback(async () => {
    if (!proposal || !detail || busy) return;

    const reason = window.prompt("Reject reason (optional):");
    if (reason === null) return;

    setBusy(true);
    try {
      const trimmed = reason.trim();
      if (trimmed) saveRejectReason(proposal.id, trimmed);
      await commitProposal(proposal.id, detail, "reject", trimmed || undefined);
      const nextId = nextQueueSelectionId(sortedQueue, proposal.id);
      setSelectedId(nextId);
      setCheckedIds((prev) => {
        const next = new Set(prev);
        next.delete(proposal.id);
        return next;
      });
      onAction();
    } finally {
      setBusy(false);
    }
  }, [proposal, detail, busy, commitProposal, sortedQueue, onAction]);

  const handleBatchApprove = useCallback(async () => {
    if (checkedIds.size === 0 || busy) return;
    setBusy(true);
    try {
      const ids = sortReviewQueue(
        sortedQueue.filter((p) => checkedIds.has(p.id)),
      ).map((p) => p.id);

      for (const id of ids) {
        const loaded = await getProposalDetail(id);
        if (!loaded) continue;
        await commitProposal(id, loaded, "accept");
      }

      setCheckedIds(new Set());
      if (proposal && checkedIds.has(proposal.id)) {
        setSelectedId(nextQueueSelectionId(sortedQueue, proposal.id));
      }
      onAction();
    } finally {
      setBusy(false);
    }
  }, [checkedIds, busy, sortedQueue, proposal, commitProposal, onAction]);

  useReviewKeyboard({
    enabled: queue.length > 0 && !busy,
    onApprove: () => void handleApprove(),
    onReject: () => void handleReject(),
    onNext: handleSelectNext,
    onPrev: handleSelectPrev,
    onFocusEditor: handleFocusEditor,
  });

  if (queue.length === 0) {
    return (
      <div className="mode-layout review-screen">
        <div className="review-empty">
          <h2>Queue clear</h2>
          <p className="placeholder">
            Librarian watching {indexed} document{indexed === 1 ? "" : "s"}.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="mode-layout review-screen review-desk">
      <ReviewQueueList
        queue={queue}
        selectedId={proposal?.id ?? null}
        checkedIds={checkedIds}
        onSelect={setSelectedId}
        onToggleChecked={handleToggleChecked}
        onBatchApprove={() => void handleBatchApprove()}
        batchBusy={busy}
      />
      <main className="review-detail">
        {proposal && (
          <>
            <div className="review-meta">
              <strong>{proposal.target_name}</strong>
              <span className="review-kind">
                {proposal.kind === "new_entity" ? "New entity" : "Update"}
              </span>
              <span className="review-model">
                Generated by {proposal.model}
              </span>
            </div>
            <ReviewProposalEditor detail={detail} containerRef={editorRef} />
            <div className="review-actions">
              <button
                className="review-btn review-btn--approve"
                onClick={() => void handleApprove()}
                disabled={busy || detail === null}
              >
                ✓ Approve
              </button>
              <button
                className="review-btn review-btn--reject"
                onClick={() => void handleReject()}
                disabled={busy || detail === null}
              >
                ✗ Reject
              </button>
              <span className="review-shortcuts-hint">
                a approve · r reject · e focus · j/k navigate · space next
              </span>
            </div>
          </>
        )}
      </main>
      {proposal && (
        <ReviewEvidencePanel
          proposal={proposal}
          reasoning={detail?.reasoning}
          onSourceClick={() => {
            /* Library navigation deferred until cross-mode routing exists */
          }}
        />
      )}
    </div>
  );
}
