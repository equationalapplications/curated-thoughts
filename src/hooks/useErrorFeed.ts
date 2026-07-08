import { useSyncExternalStore } from "react";
import {
  subscribeErrors,
  dismissError,
  retryError,
  type BackgroundError,
} from "../lib/errorFeed";

export function useErrorFeed() {
  const errors = useSyncExternalStore(subscribeErrors, () => []);
  return { errors, dismiss: dismissError, retry: retryError };
}
