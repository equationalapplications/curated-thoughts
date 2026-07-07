import { usePrivacyMode } from "../../hooks/usePrivacyMode";
import { CloudBridgePanel } from "./CloudBridgePanel";
import { PrivacyModeCards } from "../privacy/PrivacyModeCards";
import type { PrivacyMode } from "../../hooks/usePrivacyMode";

export function PrivacyPanel() {
  const { mode, setMode, loading } = usePrivacyMode();

  const handleModeChange = async (next: PrivacyMode) => {
    if (next === mode) return;

    if (mode === "connected" && next === "strict") {
      const ok = window.confirm(
        "Disconnect cloud bridge and clear remote config?",
      );
      if (!ok) return;
    } else if (mode === "connected" && next === "ephemeral") {
      const ok = window.confirm(
        "Disconnect cloud bridge and clear pairing token?",
      );
      if (!ok) return;
    }

    await setMode(next);
  };

  return (
    <div className="settings-section">
      <h3>Privacy</h3>
      <p className="settings-hint">
        Choose your data posture. This setting gates external APIs and the Cloud
        Bridge — not just how settings are displayed.
      </p>
      {loading ? (
        <p className="settings-hint">Loading privacy settings…</p>
      ) : (
        <PrivacyModeCards mode={mode} onChange={handleModeChange} />
      )}
      <CloudBridgePanel disabled={mode !== "connected"} />
    </div>
  );
}
