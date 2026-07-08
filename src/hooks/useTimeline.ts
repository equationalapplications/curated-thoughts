import { useState, useEffect, useCallback, useRef } from "react";
import { listEvents, type TimelineEvent, type TimelineFilter } from "../lib/tauri";

const POLL_MS = 5000;

export function useTimeline(filter: TimelineFilter) {
  const [events, setEvents] = useState<TimelineEvent[]>([]);
  const [error, setError] = useState<string | null>(null);
  const key = JSON.stringify(filter);
  const requestIdRef = useRef(0);

  const refresh = useCallback(() => {
    const requestId = ++requestIdRef.current;
    listEvents(JSON.parse(key) as TimelineFilter)
      .then((next) => {
        if (requestId !== requestIdRef.current) return;
        setEvents(next);
        setError(null);
      })
      .catch(() => {
        if (requestId !== requestIdRef.current) return;
        setError("Timeline is temporarily unavailable.");
      });
  }, [key]);

  useEffect(() => {
    refresh();
    const id = setInterval(refresh, POLL_MS);
    return () => clearInterval(id);
  }, [refresh]);

  return { events, error, refresh };
}
