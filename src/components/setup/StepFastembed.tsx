import { useEffect, useState } from "react";
import { initFastembed } from "../../lib/tauri";
import { onEmbedInitDone, onEmbedInitError, guardListen, safeUnlisten } from "../../lib/events";
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
    const unlistenDone = guardListen(onEmbedInitDone(() => {
      if (!mounted) return;
      onNext();
    }), "embed-init-done");
    // Registration failure means the completion event can never arrive and
    // call onNext — surface it instead of spinning in "loading" forever
    // (WizardStep keeps "Continue" disabled while isLoading is true). Side
    // branch: unlistenDone itself stays the ORIGINAL subscription (the
    // events.ts contract for caller catches), so cleanup via safeUnlisten
    // is unchanged and no derived rejection can float.
    void unlistenDone.catch((err) => {
      if (mounted) {
        setErrorMsg(String(err));
        setPhase("error");
      }
    });
    const unlistenError = guardListen(onEmbedInitError(({ message }) => {
      if (!mounted) return;
      setErrorMsg(message);
      setPhase("error");
    }), "embed-init-error");

    const setup = async () => {
      // allSettled: subscription failure degrades progress/error display but
      // must never skip the actual init (the old Promise.all skipped it).
      await Promise.allSettled([unlistenDone, unlistenError]);

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
