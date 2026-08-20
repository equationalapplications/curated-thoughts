import { useEffect, useState } from "react";
import { initFastembed } from "../../lib/tauri";
import { onEmbedInitDone, onEmbedInitError } from "../../lib/events";
import { WizardStep } from "./WizardStep";

interface Props {
  onNext: () => void;
}

type Phase = "loading" | "error";

export function StepFastembed({ onNext }: Props) {
  const [phase, setPhase] = useState<Phase>("loading");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    let unlistenDone: (() => void) | null = null;
    let unlistenError: (() => void) | null = null;
    let mounted = true;

    const setup = async () => {
      const [done, error] = await Promise.all([
        onEmbedInitDone(() => {
          if (!mounted) return;
          onNext();
        }),
        onEmbedInitError(({ message }) => {
          if (!mounted) return;
          setErrorMsg(message);
          setPhase("error");
        }),
      ]);
      unlistenDone = done;
      unlistenError = error;

      try {
        await initFastembed();
      } catch (err) {
        if (!mounted) return;
        setErrorMsg(String(err));
        setPhase("error");
      }
    };

    setup();
    return () => {
      mounted = false;
      unlistenDone?.();
      unlistenError?.();
    };
  }, [onNext]);

  return (
    <WizardStep
      title="Set up local search"
      subtitle="Initializing the vector model on first launch."
      onNext={onNext}
      nextLabel="Continue anyway"
      isLoading={phase === "loading"}
    >
      {phase === "loading" && (
        <>
          <p>Initializing vector model. This may take a moment on first launch.</p>
          <p className="ollama-hint">You can continue once the local embedding engine is ready.</p>
        </>
      )}
      {phase === "error" && (
        <>
          <p style={{ color: "red" }}>Error: {errorMsg}</p>
          <p>Search will fall back to keyword mode. You can retry from Settings.</p>
        </>
      )}
    </WizardStep>
  );
}
