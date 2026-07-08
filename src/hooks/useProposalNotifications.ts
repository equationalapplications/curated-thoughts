import { useEffect, useRef } from "react";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";

const PREF_KEY = "notify-new-proposals";

export const proposalNotificationsEnabled = () =>
  localStorage.getItem(PREF_KEY) === "true";

export const setProposalNotificationsEnabled = (on: boolean) =>
  localStorage.setItem(PREF_KEY, String(on));

export function useProposalNotifications(queueLength: number) {
  const prev = useRef<number | null>(null);

  useEffect(() => {
    const last = prev.current;
    prev.current = queueLength;

    if (last === null || queueLength <= last || !proposalNotificationsEnabled()) {
      return;
    }

    (async () => {
      try {
        let granted = await isPermissionGranted();
        if (!granted) {
          granted = (await requestPermission()) === "granted";
        }

        if (granted) {
          const n = queueLength - last;
          sendNotification({
            title: "Curated Thoughts",
            body:
              n === 1
                ? "1 new proposal awaits review."
                : `${n} new proposals await review.`,
          });
        }
      } catch (error) {
        console.error("Failed to send notification:", error);
      }
    })();
  }, [queueLength]);
}
