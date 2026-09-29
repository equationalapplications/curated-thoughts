import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ingestDocument } from "../../lib/tauri";
import {
  guardListen,
  onIngestProgress,
  onIngestProposalReady,
  onIngestError,
  safeUnlisten,
  UnlistenFn,
} from "../../lib/events";
import { WizardStep } from "./WizardStep";

type Phase = "idle" | "chunking" | "embedding" | "ready" | "error" | "stalled";

const STALL_MS = 60_000;
const STALL_POLL_MS = 1_000;

interface Props {
  onSkip: () => void;
  onRouteToReview: (proposalId: string | null) => void;
}

export function StepWatchItThink({ onSkip, onRouteToReview }: Props) {
  const [picked, setPicked] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [proposalId, setProposalId] = useState<string | null>(null);
  const lastProgressAt = useRef<number>(Date.now());
  // Mirrors `phase` so the mount-scoped stall watchdog always sees the
  // current value without re-arming its interval on every transition.
  const phaseRef = useRef<Phase>("idle");

  function applyPhase(next: Phase) {
    phaseRef.current = next;
    setPhase(next);
  }

  useEffect(() => {
    let mounted = true;
    let unlistens: Array<Promise<UnlistenFn>> = [];
    (async () => {
      unlistens = [
        guardListen(
          onIngestProgress((p) => {
            if (!mounted) return;
            lastProgressAt.current = Date.now();
            applyPhase(p.phase);
          }),
          "ingest-progress",
        ),
        guardListen(
          onIngestProposalReady((p) => {
            if (!mounted) return;
            lastProgressAt.current = Date.now();
            setProposalId(p.proposalId);
            applyPhase("ready");
          }),
          "ingest-proposal-ready",
        ),
        guardListen(
          onIngestError((p) => {
            if (!mounted) return;
            setErrorMsg(p.message);
            applyPhase("error");
          }),
          "ingest-error",
        ),
      ];
      if (!mounted) unlistens.forEach((p) => void safeUnlisten(p));
    })();
    return () => {
      mounted = false;
      unlistens.forEach((u) => void safeUnlisten(u));
    };
  }, []);

  // Stall watchdog: no progress event for 60s while the pipeline is running.
  useEffect(() => {
    const id = setInterval(() => {
      const current = phaseRef.current;
      if (current !== "chunking" && current !== "embedding") return;
      if (Date.now() - lastProgressAt.current >= STALL_MS) {
        applyPhase("stalled");
      }
    }, STALL_POLL_MS);
    return () => clearInterval(id);
  }, []);

  // Patient path: auto-route to Review once a proposal exists.
  useEffect(() => {
    if (proposalId !== null) onRouteToReview(proposalId);
  }, [proposalId, onRouteToReview]);

  async function pickFile() {
    const result = await open({
      filters: [{ name: "Documents", extensions: ["md", "txt", "pdf"] }],
    });
    if (typeof result !== "string") return; // cancelled → stay in idle
    setPicked(result);
    setErrorMsg(null);
    setProposalId(null);
    lastProgressAt.current = Date.now();
    applyPhase("chunking");
    try {
      await ingestDocument(result);
    } catch (e) {
      setErrorMsg(e instanceof Error ? e.message : String(e));
      applyPhase("error");
    }
  }

  const isRunning =
    phase === "chunking" ||
    phase === "embedding" ||
    phase === "ready" ||
    phase === "stalled";

  return (
    <WizardStep
      title="Watch it think"
      subtitle="Pick a document and follow the pipeline as it runs. This step is optional."
      onSkip={onSkip}
    >
      <div className="step-watch-it-think" data-testid="step-watch-it-think">
        {phase === "idle" && (
          /* The step's one real action. It rendered as a secondary button
             while the only other control on the step ("Skip") is a ghost, so
             the step offered no primary path at all. */
          <button
            type="button"
            className="btn btn--primary step-watch-it-think-pick"
            onClick={pickFile}
            aria-label="Choose a document to ingest"
          >
            <svg className="icon icon--sm" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
              <path d="M3.5 7.2a1.7 1.7 0 0 1 1.7-1.7h3.3l2 2.2h8.3a1.7 1.7 0 0 1 1.7 1.7v8.6a1.7 1.7 0 0 1-1.7 1.7H5.2a1.7 1.7 0 0 1-1.7-1.7z" />
            </svg>
            Choose a document to ingest
          </button>
        )}

        {isRunning && (
          <div
            className="step-watch-it-think-status"
            role="status"
            aria-live="polite"
          >
            {/* The pipeline takes up to a minute with no further output, so the
                wait needs to be visibly *working* rather than just present. */}
            <div className="step-watch-it-think-bar" aria-hidden="true">
              <span />
            </div>
            <p className="step-watch-it-think-path">{picked}</p>
            <p>
              {phase === "chunking" && "Chunking your document…"}
              {phase === "embedding" &&
                "Embedding the chunks (this can take a minute)…"}
              {phase === "ready" && "Ready — sending you to Review."}
              {phase === "stalled" &&
                "Still working… this can take a few minutes."}
            </p>
          </div>
        )}

        {phase === "error" && (
          <div className="step-watch-it-think-error" role="alert">
            <p>{errorMsg}</p>
            <button
              type="button"
              className="btn"
              onClick={() => {
                applyPhase("idle");
                setErrorMsg(null);
              }}
            >
              Try again
            </button>
          </div>
        )}
      </div>
    </WizardStep>
  );
}
