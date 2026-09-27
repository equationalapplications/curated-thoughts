import { useEffect, useState } from "react";
import { initFastembed } from "../../lib/tauri";
import { onEmbedInitDone, onEmbedInitError, safeUnlisten } from "../../lib/events";
import { WizardStep } from "./WizardStep";

interface Props {
  onNext: () => void;
}

type Phase = "loading" | "error";

export function StepFastembed({ onNext }: Props) {
  const [phase, setPhase] = useState<Phase>("loading");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    let mounted = true;
    // Hold the pending subscriptions so cleanup can remove them even when the
    // step unmounts before listen() resolves.
    const unlistenDone = onEmbedInitDone(() => {
      if (!mounted) return;
      onNext();
    });
    const unlistenError = onEmbedInitError(({ message }) => {
      if (!mounted) return;
      setErrorMsg(message);
      setPhase("error");
    });

    const setup = async () => {
      await Promise.all([unlistenDone, unlistenError]);

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
      void safeUnlisten(unlistenDone);
      void safeUnlisten(unlistenError);
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
          <p className="wizard-error">Error: {errorMsg}</p>
          <p>Search will fall back to keyword mode. You can retry from Settings.</p>
        </>
      )}
    </WizardStep>
  );
}
