import { useSyncExternalStore } from "react";
import {
  subscribeErrors,
  getErrorSnapshot,
  dismissError,
  retryError,
} from "../lib/errorFeed";

export function useErrorFeed() {
  const errors = useSyncExternalStore(subscribeErrors, getErrorSnapshot);
  return { errors, dismiss: dismissError, retry: retryError };
}
