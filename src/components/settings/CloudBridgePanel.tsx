import { useEffect, useState } from "react";
import {
  clearCloudBridgePairingToken,
  getCloudBridgeStatus,
  setCloudBridgePairingToken,
  type CloudBridgeStatus,
} from "../../lib/tauri";

const STATUS_LABEL: Record<CloudBridgeStatus["connection_status"], string> = {
  disconnected: "Not connected",
  connecting: "Connecting…",
  connected: "Connected",
  reconnecting: "Reconnecting…",
};

export function CloudBridgePanel() {
  const [status, setStatus] = useState<CloudBridgeStatus | null>(null);
  const [tokenInput, setTokenInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function refreshStatus() {
    try {
      setStatus(await getCloudBridgeStatus());
    } catch (err) {
      setError(String(err));
    }
  }

  useEffect(() => {
    let active = true;
    refreshStatus();
    const interval = setInterval(() => {
      if (active) {
        refreshStatus();
      }
    }, 3000);
    return () => {
      active = false;
      clearInterval(interval);
    };
  }, []);

  async function handleConnect() {
    if (!tokenInput.trim()) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await setCloudBridgePairingToken(tokenInput.trim());
      setTokenInput("");
      await refreshStatus();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleDisconnect() {
    setBusy(true);
    setError(null);
    try {
      await clearCloudBridgePairingToken();
      await refreshStatus();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="settings-section">
      <h3>Clanker Cloud Bridge</h3>
      <p className="vault-hint">
        Paste a pairing token from Clanker (Settings → Devices) to let a Clanker cloud agent
        query this vault. The token is query-only and stored in your OS keychain — never in
        brain.db or a config file.
      </p>

      <p className="maintenance-status" aria-live="polite">
        {status?.configured
          ? STATUS_LABEL[status.connection_status]
          : "Not paired with Clanker."}
      </p>

      {error ? (
        <p className="agent-snippet-error" role="alert" aria-live="assertive">
          {error}
        </p>
      ) : null}

      {status?.configured ? (
        <button type="button" onClick={handleDisconnect} disabled={busy}>
          Disconnect
        </button>
      ) : (
        <div className="cloud-bridge-pairing-form">
          <input
            type="password"
            value={tokenInput}
            onChange={(e) => setTokenInput(e.target.value)}
            placeholder="Paste pairing token"
            disabled={busy}
          />
          <button type="button" onClick={handleConnect} disabled={busy || !tokenInput.trim()}>
            Connect
          </button>
        </div>
      )}
    </div>
  );
}
