import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  getProposalDetail,
  resolveProposal,
  type CommitResult,
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
  queueError?: string | null;
}

function summarizeCommitResult(result: CommitResult): string | null {
  const issues: string[] = [];
  if (result.proposal_status === "partial") {
    issues.push("Applied partially");
  }
  if (result.conflicts.length > 0) {
    issues.push(`${result.conflicts.length} conflict${result.conflicts.length === 1 ? "" : "s"}`);
  }
  if (result.dropped_edges.length > 0) {
    issues.push(`${result.dropped_edges.length} dropped edge${result.dropped_edges.length === 1 ? "" : "s"}`);
  }
  return issues.length > 0 ? issues.join(" · ") : null;
}

export function ReviewMode({ queue, onAction, vaultPath, queueError }: Props) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [checkedIds, setCheckedIds] = useState<Set<string>>(() => new Set());
  const [busy, setBusy] = useState(false);
  const [detail, setDetail] = useState<ProposalDetail | null | undefined>(undefined);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionNotice, setActionNotice] = useState<string | null>(null);
  const detailRequestSeq = useRef(0);
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
    setActionError(null);
    setActionNotice(null);
    setDetail(undefined);
    if (!proposal) return;
    detailRequestSeq.current += 1;
    const requestSeq = detailRequestSeq.current;
    getProposalDetail(proposal.id)
      .then((loaded) => {
        if (detailRequestSeq.current !== requestSeq) return;
        if (loaded === null) {
          setDetail(null);
          return;
        }
        setDetail(loaded);
      })
      .catch(() => {
        if (detailRequestSeq.current === requestSeq) {
          setDetail(null);
          setActionError("Could not load proposal details.");
        }
      });
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
    ): Promise<CommitResult> => {
      const decisions =
        mode === "accept"
          ? allAcceptDecisions(loadedDetail)
          : allRejectDecisions(loadedDetail);
      return resolveProposal(
        proposalId,
        decisions,
        mode === "reject" ? rejectReason : undefined,
      );
    },
    [],
  );

  const handleApprove = useCallback(async () => {
    if (!proposal || !detail || busy) return;
    setActionError(null);
    setActionNotice(null);
    setBusy(true);
    try {
      const result = await commitProposal(proposal.id, detail, "accept");
      const notice = summarizeCommitResult(result);
      if (notice) setActionNotice(notice);
      const nextId = nextQueueSelectionId(sortedQueue, proposal.id);
      setSelectedId(nextId);
      setCheckedIds((prev) => {
        const next = new Set(prev);
        next.delete(proposal.id);
        return next;
      });
      onAction();
    } catch {
      setActionError("Could not approve proposal. Please retry.");
    } finally {
      setBusy(false);
    }
  }, [proposal, detail, busy, commitProposal, sortedQueue, onAction]);

  const handleReject = useCallback(async () => {
    if (!proposal || !detail || busy) return;

    const reason = window.prompt("Reject reason (optional):");
    if (reason === null) return;

    setActionError(null);
    setActionNotice(null);
    setBusy(true);
    try {
      const trimmed = reason.trim();
      if (trimmed) saveRejectReason(proposal.id, trimmed);
      const result = await commitProposal(
        proposal.id,
        detail,
        "reject",
        trimmed || undefined,
      );
      const notice = summarizeCommitResult(result);
      if (notice) setActionNotice(notice);
      const nextId = nextQueueSelectionId(sortedQueue, proposal.id);
      setSelectedId(nextId);
      setCheckedIds((prev) => {
        const next = new Set(prev);
        next.delete(proposal.id);
        return next;
      });
      onAction();
    } catch {
      setActionError("Could not reject proposal. Please retry.");
    } finally {
      setBusy(false);
    }
  }, [proposal, detail, busy, commitProposal, sortedQueue, onAction]);

  const handleBatchApprove = useCallback(async () => {
    if (checkedIds.size === 0 || busy) return;
    setActionError(null);
    setActionNotice(null);
    setBusy(true);
    try {
      const ids = sortReviewQueue(
        sortedQueue.filter((p) => checkedIds.has(p.id)),
      ).map((p) => p.id);
      const failedIds = new Set<string>();
      let approvedCount = 0;
      let sawPartial = false;
      let totalConflicts = 0;
      let totalDroppedEdges = 0;

      for (const id of ids) {
        try {
          const loaded = await getProposalDetail(id);
          if (!loaded) {
            failedIds.add(id);
            continue;
          }
          const result = await commitProposal(id, loaded, "accept");
          approvedCount += 1;
          if (result.proposal_status === "partial") sawPartial = true;
          totalConflicts += result.conflicts.length;
          totalDroppedEdges += result.dropped_edges.length;
        } catch {
          failedIds.add(id);
        }
      }

      setCheckedIds(failedIds);
      if (
        proposal &&
        checkedIds.has(proposal.id) &&
        !failedIds.has(proposal.id)
      ) {
        setSelectedId(nextQueueSelectionId(sortedQueue, proposal.id));
      }
      if (approvedCount > 0) {
        onAction();
      }
      const noticeParts: string[] = [];
      if (approvedCount > 0) {
        noticeParts.push(`Approved ${approvedCount}`);
      }
      if (sawPartial || totalConflicts > 0 || totalDroppedEdges > 0) {
        if (sawPartial) noticeParts.push("some partial");
        if (totalConflicts > 0) noticeParts.push(`${totalConflicts} conflict${totalConflicts === 1 ? "" : "s"}`);
        if (totalDroppedEdges > 0) noticeParts.push(`${totalDroppedEdges} dropped edge${totalDroppedEdges === 1 ? "" : "s"}`);
      }
      if (noticeParts.length > 0) {
        setActionNotice(noticeParts.join(" · "));
      }
      if (failedIds.size > 0) {
        setActionError(`${failedIds.size} proposal${failedIds.size === 1 ? "" : "s"} failed during batch approve.`);
      }
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
          <h2>{queueError ? "Queue unavailable" : "Queue clear"}</h2>
          {queueError && <p className="review-hint">{queueError}</p>}
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
            {queueError && <p className="review-hint">{queueError}</p>}
            {actionError && <p className="review-hint">{actionError}</p>}
            {actionNotice && <p className="review-hint">{actionNotice}</p>}
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
                disabled={busy || !detail}
              >
                ✓ Approve
              </button>
              <button
                className="review-btn review-btn--reject"
                onClick={() => void handleReject()}
                disabled={busy || !detail}
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
          items={detail?.items}
          onSourceClick={() => {
            /* Library navigation deferred until cross-mode routing exists */
          }}
        />
      )}
    </div>
  );
}
