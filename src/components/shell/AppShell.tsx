import { useCallback, useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ModeRail, AppMode } from "./ModeRail";
import { StatusBar } from "./StatusBar";
import { ActivityFeedPanel } from "./ActivityFeedPanel";
import { PeekPanel, type PeekTarget } from "./PeekPanel";
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
import { startFileWatcher, needsChunkHashMigration } from "../../lib/tauri";
import { onVaultSwitched } from "../../lib/events";
import { reportBackgroundError } from "../../lib/errorFeed";
import { useProposalQueue } from "../../hooks/useProposalQueue";
import { useProposalNotifications } from "../../hooks/useProposalNotifications";
import { usePrivacyMode } from "../../hooks/usePrivacyMode";
import { useErrorFeed } from "../../hooks/useErrorFeed";
import { useNavigationState } from "../../lib/navigation";
import { MigrationDisclosureModal } from "../privacy/MigrationDisclosureModal";
import { SplashScreen } from "./SplashScreen";

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
  const [peekTarget, setPeekTarget] = useState<PeekTarget | null>(null);
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
  // `null` while we're checking the backend gate; `true` once we know no
  // migration is needed (or it has already completed); `false` while we
  // still need to mount the splash and wait for `migration-complete`.
  // Defaults to `true` (skips splash) in environments without the IPC
  // bridge so the app still renders for tests/storybooks that mock the
  // command away.
  const [migrationComplete, setMigrationComplete] = useState<boolean | null>(true);

  useEffect(() => {
    let active = true;
    needsChunkHashMigration()
      .then((needed) => {
        if (!active) return;
        // `needed === true` → mount splash and wait for `migration-complete`
        // to flip this back. `needed === false` → the migration is done
        // (or never had work to do), so the rest of the UI is safe to show.
        setMigrationComplete(!needed);
      })
      .catch(() => {
        // On gate-query failure default to "no migration needed" — the
        // app should still render rather than hang on a stuck splash.
        // The startup migration in `lib.rs` already handles its own
        // error path and emits `migration-error` if it fails.
        if (active) setMigrationComplete(true);
      });
    return () => {
      active = false;
    };
  }, []);

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

  function handlePeekSource(path: string, chunkId: string | null) {
    // Unreachable via FactCard's dispatch rule (it never calls onPeekSource
    // with a null hash); the guard keeps PeekTarget.hash: string honest.
    if (!chunkId) return;
    setPeekTarget({ path, hash: chunkId });
  }

  function handlePromote(path: string, hash: string) {
    setPeekTarget(null);
    nav.navigate({ mode: "library", docPath: path, chunkId: hash });
  }

  useEffect(() => {
    const promise = onVaultSwitched((newPath) => {
      setPeekTarget(null);
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
      {migrationComplete === false ? (
        <SplashScreen onComplete={() => setMigrationComplete(true)} />
      ) : migrationComplete === null ? (
        // Gate query hasn't returned yet; render an empty shell so the
        // OS window has something to attach to while we wait.
        null
      ) : (
        <>
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
                      onPeekSource={handlePeekSource}
                      onEntityName={setBrainEntityName}
                      onGoToLibrary={() => nav.navigate({ mode: "library" })}
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
                      onPickFile={() => nav.navigate({ mode: "setup" })}
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
          <PeekPanel
            target={peekTarget}
            onDismiss={() => setPeekTarget(null)}
            onPromote={handlePromote}
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
        </>
      )}
    </div>
  );
}
