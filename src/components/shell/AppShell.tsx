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
  const [mode, setMode] = useState<AppMode>("brain");
  const [settingsTab, setSettingsTab] = useState<SettingsTab | undefined>();
  const [activityOpen, setActivityOpen] = useState(false);
  const [brainDoc, setBrainDoc] = useState<string | null>(null);
  const [libraryDoc, setLibraryDoc] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const { queue, refresh } = useProposalQueue(vaultPath);

  useEffect(() => {
    startFileWatcher().catch(console.error);
  }, [vaultPath]);

  useEffect(() => {
    const promise = onVaultSwitched((newPath) => {
      setBrainDoc(null);
      setLibraryDoc(null);
      setMode("brain");
      onVaultChanged(newPath);
    });
    return () => {
      promise.then((unlisten) => unlisten());
    };
  }, [onVaultChanged]);

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
        setMode(target);
      }
      if (e.key === "k") {
        e.preventDefault();
        // Command palette ships in a later phase.
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  useEffect(() => {
    const focused =
      mode === "brain"
        ? docTitleSegment(brainDoc)
        : mode === "library"
          ? docTitleSegment(libraryDoc)
          : null;
    const title = focused
      ? `${MODE_TITLES[mode]} — ${focused}`
      : MODE_TITLES[mode];
    getCurrentWindow()
      .setTitle(`Curated Thoughts — ${title}`)
      .catch(() => {});
  }, [mode, brainDoc, libraryDoc]);

  useEffect(() => {
    if (!activityOpen) return;
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape") setActivityOpen(false);
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [activityOpen]);

  function openPrivacySettings() {
    setSettingsTab("privacy");
    setMode("settings");
  }

  return (
    <div className="app-root">
      <div className="app-body">
        <ModeRail
          mode={mode}
          reviewCount={queue.length}
          onModeChange={(next) => {
            setSettingsTab(undefined);
            setMode(next);
          }}
        />
        <div className="app-main">
          {mode === "brain" && (
            <BrainMode
              vaultPath={vaultPath}
              selectedDoc={brainDoc}
              onDocSelect={setBrainDoc}
            />
          )}
          {mode === "review" && (
            <ReviewMode queue={queue} onAction={refresh} vaultPath={vaultPath} />
          )}
          {mode === "library" && (
            <LibraryMode
              vaultPath={vaultPath}
              selectedDoc={libraryDoc}
              onDocSelect={setLibraryDoc}
            />
          )}
          {mode === "settings" && (
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
    </div>
  );
}
