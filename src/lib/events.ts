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
