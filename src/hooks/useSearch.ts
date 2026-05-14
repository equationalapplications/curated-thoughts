import { useMemoryRead } from "./useMemoryRead";

export function useSearch(vaultPath: string) {
  return useMemoryRead(vaultPath);
}
