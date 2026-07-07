import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ModeRail, AppMode } from "./ModeRail";
import { StatusBar } from "./StatusBar";
import { ActivityFeedPanel } from "./ActivityFeedPanel";
import { BrainMode } from "../modes/BrainMode";
import { LibraryMode } from "../modes/LibraryMode";
import { ReviewMode } from "../modes/ReviewMode";
import {
  SettingsScreen,
  type SettingsTab,
} from "../settings/SettingsScreen";
import { startFileWatcher } from "../../lib/tauri";
import { onVaultSwitched } from "../../lib/events";
import { useProposalQueue } from "../../hooks/useProposalQueue";
import { usePrivacyMode } from "../../hooks/usePrivacyMode";
import { useNavigationState } from "../../lib/navigation";
import { MigrationDisclosureModal } from "../privacy/MigrationDisclosureModal";

interface Props {
  vaultPath: string;
  onVaultChanged: (newPath: string) => void;
}

const MODE_SHORTCUTS: Record<string, AppMode> = {
  "1": "brain",
  "2": "review",
  "3": "library",
};

const MODE_TITLES: Record<AppMode, string> = {
  brain: "Brain",
  review: "Review",
  library: "Library",
  settings: "Settings",
};

function docTitleSegment(path: string | null): string | null {
  if (!path) return null;
  return path.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) ?? path;
}

export function AppShell({ vaultPath, onVaultChanged }: Props) {
  const nav = useNavigationState({ mode: "brain" });
  const [settingsTab, setSettingsTab] = useState<SettingsTab | undefined>();
  const [activityOpen, setActivityOpen] = useState(false);
  const [brainEntityId, setBrainEntityId] = useState<string | null>(null);
  const [brainEntityName, setBrainEntityName] = useState<string | null>(null);
  const [libraryDoc, setLibraryDoc] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const { queue, refresh, error: queueError } = useProposalQueue(vaultPath);
  const {
    needs_migration_disclosure,
    loading: privacyLoading,
    mode: privacyMode,
  } = usePrivacyMode();
  const [migrationDismissed, setMigrationDismissed] = useState(false);

  useEffect(() => {
    startFileWatcher().catch(console.error);
  }, [vaultPath]);

  useEffect(() => {
    const promise = onVaultSwitched((newPath) => {
      setBrainEntityId(null);
      setBrainEntityName(null);
      setLibraryDoc(null);
      nav.navigate({ mode: "brain" });
      onVaultChanged(newPath);
    });
    return () => {
      promise.then((unlisten) => unlisten());
    };
  }, [onVaultChanged, nav]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    getCurrentWindow()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "leave") {
          setDragging(false);
          return;
        }
        if (payload.type === "enter" || payload.type === "over") {
          setDragging(true);
        } else if (payload.type === "drop") {
          setDragging(false);
        }
      })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [vaultPath]);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (!(e.metaKey || e.ctrlKey)) return;
      const target = MODE_SHORTCUTS[e.key];
      if (target) {
        e.preventDefault();
        nav.navigate({ mode: target });
      }
      if (e.key === "k") {
        e.preventDefault();
        // Command palette ships in a later phase.
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [nav]);

  useEffect(() => {
    const focused =
      nav.current.mode === "brain"
        ? brainEntityName
        : nav.current.mode === "library"
          ? docTitleSegment(libraryDoc)
          : null;
    const title = focused
      ? `${MODE_TITLES[nav.current.mode]} — ${focused}`
      : MODE_TITLES[nav.current.mode];
    getCurrentWindow()
      .setTitle(`Curated Thoughts — ${title}`)
      .catch(() => {});
  }, [nav.current.mode, brainEntityName, libraryDoc]);

  useEffect(() => {
    if (!activityOpen) return;
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape") setActivityOpen(false);
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [activityOpen]);

  useEffect(() => {
    if (nav.current.mode === "brain") {
      setBrainEntityId(nav.current.entityId ?? null);
      setBrainEntityName(null);
    } else if (nav.current.mode === "library") {
      setLibraryDoc(nav.current.docPath ?? null);
    }
  }, [nav.current.mode, nav.current.entityId, nav.current.docPath]);

  function openPrivacySettings() {
    setSettingsTab("privacy");
    nav.navigate({ mode: "settings" });
  }

  return (
    <div className="app-root">
      <div className="app-body">
        <ModeRail
          mode={nav.current.mode}
          reviewCount={queue.length}
          canGoBack={nav.canGoBack}
          canGoForward={nav.canGoForward}
          onModeChange={(next) => {
            setSettingsTab(undefined);
            nav.navigate({ mode: next });
          }}
          onBack={nav.goBack}
          onForward={nav.goForward}
        />
        <div className="app-main">
          {nav.current.mode === "brain" && (
            <BrainMode
              selectedEntityId={brainEntityId}
              onEntitySelect={(id) => {
                if (!id) {
                  setBrainEntityId(null);
                  setBrainEntityName(null);
                } else {
                  nav.navigate({ mode: "brain", entityId: id });
                }
              }}
              onOpenSource={(path) => {
                nav.navigate({ mode: "library", docPath: path });
              }}
              onEntityName={setBrainEntityName}
            />
          )}
          {nav.current.mode === "review" && (
            <ReviewMode
              queue={queue}
              onAction={refresh}
              vaultPath={vaultPath}
              queueError={queueError}
              onOpenSource={(path) => {
                nav.navigate({ mode: "library", docPath: path });
              }}
            />
          )}
          {nav.current.mode === "library" && (
            <LibraryMode
              vaultPath={vaultPath}
              selectedDoc={libraryDoc}
              onDocSelect={setLibraryDoc}
            />
          )}
          {nav.current.mode === "settings" && (
            <SettingsScreen vaultPath={vaultPath} initialTab={settingsTab} />
          )}
        </div>
      </div>
      <StatusBar
        vaultPath={vaultPath}
        onOpenActivity={() => setActivityOpen(true)}
        onOpenPrivacy={openPrivacySettings}
      />
      <ActivityFeedPanel
        open={activityOpen}
        onClose={() => setActivityOpen(false)}
      />
      {dragging && (
        <div className="drop-overlay">
          <span>Drop to add to Library</span>
        </div>
      )}
      {!privacyLoading &&
      needs_migration_disclosure &&
      !migrationDismissed &&
      privacyMode === "connected" ? (
        <MigrationDisclosureModal
          onAcknowledged={() => setMigrationDismissed(true)}
        />
      ) : null}
    </div>
  );
}
