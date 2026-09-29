import { listen, UnlistenFn } from "@tauri-apps/api/event";

export type { UnlistenFn };

// Backoff between unlisten attempts (~0.5s in total), long enough for a
// registration eval queued behind startup work in a busy webview.
const UNLISTEN_RETRY_DELAYS_MS = [0, 1, 2, 4, 8, 16, 32, 64, 128, 256];

/**
 * Unsubscribe a Tauri listener without racing its registration.
 *
 * Tauri registers the JS side of a listener by eval'ing a script into the
 * webview, but resolves `listen()` over a separate IPC channel, so `listen()`
 * can resolve before the registration lands. Unlistening in that window (an
 * effect cleaned up right after mount) throws inside `unregisterListener`
 * (`listeners[eventId].handlerId`) before the Rust side is told, which leaks
 * the listener and surfaces as an unhandled rejection. Retry with backoff
 * until the registration has landed.
 */
export async function safeUnlisten(
  unlisten: UnlistenFn | Promise<UnlistenFn> | undefined,
): Promise<void> {
  let fn: UnlistenFn | undefined;
  try {
    fn = await unlisten;
  } catch {
    return; // listen() never subscribed, so there is nothing to remove
  }
  if (!fn) return;
  for (const delay of [...UNLISTEN_RETRY_DELAYS_MS, null]) {
    try {
      await fn();
      return;
    } catch (err) {
      if (delay === null) {
        console.warn("[events] unlisten failed", err);
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, delay));
    }
  }
}

/**
 * Attach a logging rejection handler to a pending Tauri subscription so a
 * failed listen() surfaces as ONE console.warn instead of an
 * unhandledrejection. Returns the ORIGINAL promise — pass it to
 * safeUnlisten() exactly as before (safeUnlisten already tolerates a
 * rejected listen). Callers may add their OWN non-logging .catch for UI
 * degradation; never add a second LOGGING handler (double-log).
 */
export function guardListen(
  subscription: Promise<UnlistenFn>,
  context: string,
): Promise<UnlistenFn> {
  subscription.catch((err) => {
    console.warn(`[events] listen failed (${context})`, err);
  });
  return subscription;
}

export interface VaultEvent {
  kind: "Added" | "Modified" | "Deleted";
  path: string;
}

export interface PullProgress {
  completed: number;
  total: number;
}

export const onVaultEvent = (
  cb: (event: VaultEvent) => void
): Promise<UnlistenFn> =>
  listen<VaultEvent>("vault-event", (e) => cb(e.payload));

export const onPullProgress = (
  cb: (progress: PullProgress) => void
): Promise<UnlistenFn> =>
  listen<PullProgress>("ollama-pull-progress", (e) => cb(e.payload));

export const onVaultSwitched = (
  cb: (newPath: string) => void
): Promise<UnlistenFn> =>
  listen<string>("vault-switched", (e) => cb(e.payload));

export const onSidecarDownloadProgress = (
  cb: (progress: DownloadProgress) => void
): Promise<UnlistenFn> =>
  listen<DownloadProgress>("sidecar-download-progress", (e) => cb(e.payload));

export interface ProviderLoading {
  elapsed_s: number;
}

export interface DownloadProgress {
  downloaded: number;
  total: number;
}

export interface ErrorPayload {
  message: string;
}

export const onProviderLoading = (
  cb: (payload: ProviderLoading) => void
): Promise<UnlistenFn> =>
  listen<ProviderLoading>("provider-loading", (e) => cb(e.payload));

export const onProviderReady = (cb: () => void): Promise<UnlistenFn> =>
  listen<void>("provider-ready", () => cb());

export const onProviderError = (
  cb: (payload: ErrorPayload) => void
): Promise<UnlistenFn> =>
  listen<ErrorPayload>("provider-error", (e) => cb(e.payload));

export const onEmbedInitProgress = (cb: () => void): Promise<UnlistenFn> =>
  listen<void>("embed-init-progress", () => cb());

export const onEmbedInitDone = (cb: () => void): Promise<UnlistenFn> =>
  listen<void>("embed-init-done", () => cb());

export const onEmbedInitError = (
  cb: (payload: ErrorPayload) => void
): Promise<UnlistenFn> =>
  listen<ErrorPayload>("embed-init-error", (e) => cb(e.payload));

export const onGgufDownloadProgress = (
  cb: (progress: DownloadProgress) => void
): Promise<UnlistenFn> =>
  listen<DownloadProgress>("gguf-download-progress", (e) => cb(e.payload));

export type IngestPhase = "chunking" | "embedding" | "ready";

export interface IngestProgress {
  phase: IngestPhase;
  path: string;
}

export interface IngestProposalReady {
  path: string;
  proposalId: string | null;
}

export const onIngestProgress = (
  cb: (progress: IngestProgress) => void,
): Promise<UnlistenFn> =>
  listen<IngestProgress>("ingest-progress", (e) => cb(e.payload));

export const onIngestProposalReady = (
  cb: (payload: IngestProposalReady) => void,
): Promise<UnlistenFn> =>
  listen<IngestProposalReady>("ingest-proposal-ready", (e) => cb(e.payload));

export const onIngestError = (
  cb: (payload: ErrorPayload) => void,
): Promise<UnlistenFn> =>
  listen<ErrorPayload>("ingest-error", (e) => cb(e.payload));
