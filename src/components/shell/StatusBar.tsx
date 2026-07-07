import { useIndexingStatus } from "../../hooks/useIndexingStatus";
import { useProviderHealth, type HealthState } from "../../hooks/useProviderHealth";
import { usePrivacyMode } from "../../hooks/usePrivacyMode";
import { useVaultSwitcher } from "../../hooks/useVaultSwitcher";
import { useWikiStatus } from "../../hooks/useWikiStatus";
import { PrivacyShieldIcon } from "../privacy/PrivacyShieldIcon";

interface Props {
  vaultPath: string;
  onOpenActivity: () => void;
  onOpenPrivacy: () => void;
}

function vaultName(vaultPath: string): string {
  return (
    vaultPath.replace(/\\/g, "/").replace(/\/+$/, "").split("/").at(-1) ??
    vaultPath
  );
}

function librarianLabel(
  indexed: number,
  pending: number,
  wikiBusy: boolean,
  wikiLabel: string | null,
  librarian: boolean,
  ingesting: boolean,
): string {
  if (pending > 0) {
    return `Embedding ${pending} file${pending === 1 ? "" : "s"}…`;
  }
  if (librarian) return "Synthesizing…";
  if (ingesting) return "Ingesting…";
  if (wikiBusy && wikiLabel) return `${wikiLabel}…`;
  if (indexed > 0) {
    return `Idle — ${indexed} doc${indexed === 1 ? "" : "s"} indexed`;
  }
  return "Idle";
}

function healthTitle(kind: "Generation" | "Embeddings", state: HealthState): string {
  const labels: Record<HealthState, string> = {
    ok: "ready",
    loading: "starting",
    error: "error",
    unconfigured: "not configured",
  };
  return `${kind}: ${labels[state]}`;
}

import type { PrivacyMode } from "../../hooks/usePrivacyMode";

function privacyLabel(mode: PrivacyMode): string {
  if (mode === "strict") return "Strict privacy — local only";
  if (mode === "ephemeral") return "Ephemeral cloud inference";
  return "Connected agent — Cloud Bridge";
}

export function StatusBar({ vaultPath, onOpenActivity, onOpenPrivacy }: Props) {
  const { indexed, pending } = useIndexingStatus(vaultPath);
  const wikiStatus = useWikiStatus();
  const { generation, embedding } = useProviderHealth();
  const { mode: privacyMode } = usePrivacyMode();
  const { changeVault, switching } = useVaultSwitcher(vaultPath);

  const librarianText = librarianLabel(
    indexed,
    pending,
    wikiStatus.busy,
    wikiStatus.activeJobLabel,
    wikiStatus.librarian,
    wikiStatus.ingesting,
  );
  const busy = pending > 0 || wikiStatus.busy;

  return (
    <footer className="status-bar">
      <button
        type="button"
        className={`status-bar-segment status-bar-segment--left${
          busy ? " status-bar-segment--busy" : ""
        }`}
        onClick={onOpenActivity}
        title="Open activity feed"
      >
        {librarianText}
      </button>

      <div className="status-bar-segment status-bar-segment--center">
        <button
          type="button"
          className="status-bar-health"
          onClick={onOpenActivity}
          title="Open activity feed"
          aria-label="Model and embedder health"
        >
          <span
            className={`status-dot status-dot--${generation}`}
            title={healthTitle("Generation", generation)}
          />
          <span
            className={`status-dot status-dot--${embedding}`}
            title={healthTitle("Embeddings", embedding)}
          />
        </button>
        <button
          type="button"
          className="status-bar-privacy"
          onClick={onOpenPrivacy}
          title={privacyLabel(privacyMode)}
          aria-label={privacyLabel(privacyMode)}
        >
          <PrivacyShieldIcon mode={privacyMode} />
        </button>
      </div>

      <button
        type="button"
        className="status-bar-segment status-bar-segment--right"
        onClick={() => changeVault()}
        disabled={switching}
        title={vaultPath}
        aria-label={`Vault: ${vaultName(vaultPath)}. Click to switch.`}
      >
        {switching ? "Switching…" : vaultName(vaultPath)}
      </button>
    </footer>
  );
}
