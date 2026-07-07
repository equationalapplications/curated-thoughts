import { useState } from "react";
import { usePrivacyMode } from "../../hooks/usePrivacyMode";
import { PrivacyModeCards } from "../privacy/PrivacyModeCards";
import type { PrivacyMode } from "../../hooks/usePrivacyMode";

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
    <div className="setup-step">
      <h2>Choose your privacy posture</h2>
      <p>
        This controls whether external APIs and the Clanker Cloud Bridge can run.
        You can change it later in Settings → Privacy.
      </p>
      {loading ? (
        <p>Loading…</p>
      ) : (
        <PrivacyModeCards mode={selected} onChange={setSelected} />
      )}
      <button type="button" onClick={handleContinue} disabled={busy || loading}>
        Continue
      </button>
    </div>
  );
}
