import { useCallback, useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ModeRail, AppMode } from "./ModeRail";
import { StatusBar } from "./StatusBar";
import { ActivityFeedPanel } from "./ActivityFeedPanel";
import { BrainMode } from "../modes/BrainMode";
import { LibraryMode } from "../modes/LibraryMode";
import { ReviewMode } from "../modes/ReviewMode";
import { TimelineMode } from "../modes/TimelineMode";
import { TasksMode } from "../modes/TasksMode";
import {
  SettingsScreen,
  type SettingsTab,
} from "../settings/SettingsScreen";
import { SetupWizard } from "../setup/SetupWizard";
import { startFileWatcher } from "../../lib/tauri";
import { onVaultSwitched } from "../../lib/events";
import { reportBackgroundError } from "../../lib/errorFeed";
import { useProposalQueue } from "../../hooks/useProposalQueue";
import { useProposalNotifications } from "../../hooks/useProposalNotifications";
import { usePrivacyMode } from "../../hooks/usePrivacyMode";
import { useErrorFeed } from "../../hooks/useErrorFeed";
import { useNavigationState } from "../../lib/navigation";
import { MigrationDisclosureModal } from "../privacy/MigrationDisclosureModal";

interface Props {
  vaultPath: string;
  onVaultChanged: (newPath: string) => void;
  needsSetup: boolean;
}

const MODE_SHORTCUTS: Record<string, AppMode> = {
  "1": "brain",
  "2": "review",
  "3": "library",
  "4": "timeline",
  "5": "tasks",
};

const MODE_TITLES: Record<AppMode, string> = {
  brain: "Brain",
  review: "Review",
  library: "Library",
  timeline: "Timeline",
  tasks: "Tasks",
  settings: "Settings",
  setup: "Setup",
};

function docTitleSegment(path: string | null): string | null {
  if (!path) return null;
  return path.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) ?? path;
}

export function AppShell({ vaultPath, onVaultChanged, needsSetup }: Props) {
  const nav = useNavigationState({ mode: "brain" });
  const [settingsTab, setSettingsTab] = useState<SettingsTab | undefined>();
  const [activityOpen, setActivityOpen] = useState(false);
  const [brainEntityId, setBrainEntityId] = useState<string | null>(null);
  const [brainEntityName, setBrainEntityName] = useState<string | null>(null);
  const [libraryDoc, setLibraryDoc] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const { queue, refresh, error: queueError } = useProposalQueue(vaultPath);
  useProposalNotifications(queue.length);
  const wizardActive = needsSetup || nav.current.mode === "setup";
  const {
    needs_migration_disclosure,
    loading: privacyLoading,
    mode: privacyMode,
  } = usePrivacyMode();
  const { errors } = useErrorFeed();
  const [migrationDismissed, setMigrationDismissed] = useState(false);

  useEffect(() => {
    const start = () =>
      startFileWatcher().catch(() => {
        reportBackgroundError(
          "File watcher failed to start — new documents won't be detected.",
          start
        );
      });
    start();
  }, [vaultPath]);

  useEffect(() => {
    const promise = onVaultSwitched((newPath) => {
      setBrainEntityId(null);
      setBrainEntityName(null);
      setLibraryDoc(null);
      nav.reset({ mode: "brain" });
      onVaultChanged(newPath);
    });
    return () => {
      promise.then((unlisten) => unlisten());
    };
  }, [onVaultChanged, nav.reset]);

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
  }, [nav.navigate]);

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

  const onRouteToReview = useCallback(
    (proposalId: string | null) =>
      proposalId
        ? nav.navigate({ mode: "review", proposalId })
        : nav.navigate({ mode: "review" }),
    [nav.navigate],
  );

  function openPrivacySettings() {
    setSettingsTab("privacy");
    nav.navigate({ mode: "settings" });
  }

  return (
    <div className="app-root">
      <div className="app-body">
        {wizardActive ? (
          <SetupWizard
            onComplete={() => {
              nav.navigate({ mode: "brain" });
            }}
            initialStep={0}
            vaultPath={vaultPath}
            onRouteToReview={onRouteToReview}
          />
        ) : (
          <>
            <ModeRail
              mode={nav.current.mode}
              reviewCount={queue.length}
              errorCount={errors.length}
              canGoBack={nav.canGoBack}
              canGoForward={nav.canGoForward}
              onModeChange={(next) => {
                setSettingsTab(undefined);
                nav.navigate({ mode: next });
              }}
              onBack={nav.goBack}
              onForward={nav.goForward}
              onOpenActivity={() => setActivityOpen(true)}
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
                  onOpenSource={(path, chunkId) => {
                    nav.navigate({ mode: "library", docPath: path, chunkId: chunkId ?? undefined });
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
                  onOpenSource={(path, chunkId) => {
                    nav.navigate({ mode: "library", docPath: path, chunkId: chunkId ?? undefined });
                  }}
                />
              )}
              {nav.current.mode === "library" && (
                <LibraryMode
                  vaultPath={vaultPath}
                  selectedDoc={libraryDoc}
                  anchorChunkId={nav.current.chunkId ?? null}
                  onDocSelect={(path) => path ? nav.navigate({ mode: "library", docPath: path }) : setLibraryDoc(null)}
                />
              )}
              {nav.current.mode === "timeline" && (
                <TimelineMode onNavigate={nav.navigate} />
              )}
              {nav.current.mode === "tasks" && (
                <TasksMode onNavigate={nav.navigate} />
              )}
              {nav.current.mode === "settings" && (
                <SettingsScreen vaultPath={vaultPath} initialTab={settingsTab} onRerunWizard={() => nav.navigate({ mode: "setup" })} />
              )}
            </div>
          </>
        )}
      </div>
      <StatusBar
        vaultPath={vaultPath}
        onOpenActivity={() => setActivityOpen(true)}
        onOpenPrivacy={openPrivacySettings}
      />
      <ActivityFeedPanel
        isOpen={activityOpen}
        onClose={() => setActivityOpen(false)}
        onNavigate={(t) => {
          nav.navigate(t);
          setActivityOpen(false);
        }}
        errors={errors}
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
