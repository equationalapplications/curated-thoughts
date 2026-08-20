import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ingestDocument } from "../../lib/tauri";
import {
  onIngestProgress,
  onIngestProposalReady,
  onIngestError,
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
    let unlistens: Array<() => void> = [];
    (async () => {
      const [up, ur, ue] = await Promise.all([
        onIngestProgress((p) => {
          if (!mounted) return;
          lastProgressAt.current = Date.now();
          applyPhase(p.phase);
        }),
        onIngestProposalReady((p) => {
          if (!mounted) return;
          lastProgressAt.current = Date.now();
          setProposalId(p.proposalId);
          applyPhase("ready");
        }),
        onIngestError((p) => {
          if (!mounted) return;
          setErrorMsg(p.message);
          applyPhase("error");
        }),
      ]);
      unlistens = [up, ur, ue];
      if (!mounted) unlistens.forEach((u) => u());
    })();
    return () => {
      mounted = false;
      unlistens.forEach((u) => u());
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
          <button
            type="button"
            className="step-watch-it-think-pick"
            onClick={pickFile}
            aria-label="Choose a document to ingest"
          >
            Choose a document to ingest
          </button>
        )}

        {isRunning && (
          <div
            className="step-watch-it-think-status"
            role="status"
            aria-live="polite"
          >
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
