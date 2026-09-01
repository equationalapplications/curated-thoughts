import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

export type Politeness = "polite" | "assertive";

const FLOOR_MS = 150;

interface Entry {
  id: number;
  text: string;
  at: number;
}

interface Channel {
  displayed: Entry | null;
  queue: Entry[];
  lastWriteAt: number;
  drainTimer: number | null;
}

const emptyChannel = (): Channel => ({
  displayed: null,
  queue: [],
  lastWriteAt: 0,
  drainTimer: null,
});

interface AnnouncerContextValue {
  announce: (text: string, politeness?: Politeness) => void;
}

const AnnouncerContext = createContext<AnnouncerContextValue | null>(null);

export function useAnnouncer(): AnnouncerContextValue {
  const ctx = useContext(AnnouncerContext);
  if (!ctx) {
    throw new Error("useAnnouncer requires <AnnouncerProvider>");
  }
  return ctx;
}

export function AnnouncerProvider({ children }: { children: ReactNode }) {
  const [politeEntry, setPoliteEntry] = useState<Entry | null>(null);
  const [assertiveEntry, setAssertiveEntry] = useState<Entry | null>(null);
  const nextIdRef = useRef(0);
  const channelsRef = useRef<Record<Politeness, Channel>>({
    polite: emptyChannel(),
    assertive: emptyChannel(),
  });
  // Every pending timeout id, self-removing on fire — compacted, so this set
  // never grows with announcement history (audit nit, fixed 2026-09-01).
  const liveTimersRef = useRef<Set<number>>(new Set());

  useEffect(() => {
    const live = liveTimersRef.current;
    return () => {
      for (const t of live) window.clearTimeout(t);
      live.clear();
    };
  }, []);

  /** DOM write: display the next queued entry for its 150ms floor. */
  const writeNext = useCallback((politeness: Politeness) => {
    const ch = channelsRef.current[politeness];
    const next = ch.queue.shift();
    if (!next) return;
    const live = liveTimersRef.current;
    ch.lastWriteAt = Date.now();
    ch.displayed = next;
    (politeness === "assertive" ? setAssertiveEntry : setPoliteEntry)(next);
    const removal = window.setTimeout(() => {
      live.delete(removal);
      (politeness === "assertive" ? setAssertiveEntry : setPoliteEntry)(
        (prev) => (prev?.id === next.id ? null : prev),
      );
    }, FLOOR_MS);
    live.add(removal);
  }, []);

  /**
   * Sequential drain (spec §3): DOM writes are spaced at least FLOOR_MS
   * apart. Colliding messages queue and drain one per floor interval; a
   * message arriving on an idle channel is written immediately.
   */
  const scheduleDrain = useCallback((politeness: Politeness) => {
    const ch = channelsRef.current[politeness];
    if (ch.drainTimer !== null) return;
    const delay = Math.max(ch.lastWriteAt + FLOOR_MS - Date.now(), 0);
    const live = liveTimersRef.current;
    const timer = window.setTimeout(() => {
      live.delete(timer);
      ch.drainTimer = null;
      if (ch.queue.length === 0) return;
      writeNext(politeness);
      scheduleDrain(politeness);
    }, delay);
    ch.drainTimer = timer;
    live.add(timer);
  }, [writeNext]);

  const announce = useCallback(
    (text: string, politeness: Politeness = "polite") => {
      const ch = channelsRef.current[politeness];
      const at = Date.now();
      // FIFO append; identical-message collapsing applies within the 150ms
      // floor window of the immediately preceding message on THIS politeness
      // channel — cross-channel duplicates are still read twice (spec:
      // announcer — duplicates never read twice within a channel).
      const last = ch.queue[ch.queue.length - 1] ?? ch.displayed;
      if (last && last.text === text && at - last.at < FLOOR_MS) return;
      ch.queue.push({ id: nextIdRef.current++, text, at });
      // Lone message on an idle channel: write immediately (spec — no
      // latency penalty for the common single-message case). Otherwise the
      // drain scheduler spaces writes.
      if (ch.drainTimer === null && Date.now() - ch.lastWriteAt >= FLOOR_MS) {
        writeNext(politeness);
        scheduleDrain(politeness);
      }
      if (ch.drainTimer === null && ch.queue.length > 0) scheduleDrain(politeness);
    },
    [scheduleDrain, writeNext],
  );

  const value = useMemo(() => ({ announce }), [announce]);

  return (
    <AnnouncerContext.Provider value={value}>
      {children}
      <div aria-live="polite" aria-atomic="true" className="a11y-announcer">
        {politeEntry && <p key={politeEntry.id}>{politeEntry.text}</p>}
      </div>
      <div role="alert" aria-live="assertive" aria-atomic="true" className="a11y-announcer">
        {assertiveEntry && <p key={assertiveEntry.id}>{assertiveEntry.text}</p>}
      </div>
    </AnnouncerContext.Provider>
  );
}
