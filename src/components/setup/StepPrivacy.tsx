import { useState } from "react";
import { usePrivacyMode } from "../../hooks/usePrivacyMode";
import { PrivacyModeCards } from "../privacy/PrivacyModeCards";
import type { PrivacyMode } from "../../hooks/usePrivacyMode";
import { WizardStep } from "./WizardStep";

interface Props {
  onNext: () => void;
}

export function StepPrivacy({ onNext }: Props) {
  const { mode, setMode, loading } = usePrivacyMode();
  const [selected, setSelected] = useState<PrivacyMode>(mode);
  const [busy, setBusy] = useState(false);

  const handleContinue = async () => {
    setBusy(true);
    try {
      await setMode(selected);
      onNext();
    } finally {
      setBusy(false);
    }
  };

  return (
    <WizardStep
      title="Choose your privacy posture"
      subtitle="This controls whether external APIs and the Clanker Cloud Bridge can run. You can change it later in Settings → Privacy."
      onNext={handleContinue}
      nextDisabled={busy || loading || !selected}
      isLoading={busy}
    >
      {loading ? (
        <p>Loading…</p>
      ) : (
        <PrivacyModeCards mode={selected} onChange={setSelected} />
      )}
    </WizardStep>
  );
}
