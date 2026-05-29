import { listen, UnlistenFn } from "@tauri-apps/api/event";

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
