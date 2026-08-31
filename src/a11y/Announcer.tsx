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

interface QueueEntry {
  id: number;
  text: string;
  at: number;
}

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
  const [polite, setPolite] = useState<QueueEntry[]>([]);
  const [assertive, setAssertive] = useState<QueueEntry[]>([]);
  const nextIdRef = useRef(0);
  const timersRef = useRef<number[]>([]);

  useEffect(() => {
    const timers = timersRef.current;
    return () => {
      for (const t of timers) window.clearTimeout(t);
    };
  }, []);

  const announce = useCallback((text: string, politeness: Politeness = "polite") => {
    const id = nextIdRef.current++;
    const at = Date.now();
    const setter = politeness === "assertive" ? setAssertive : setPolite;
    setter((queue) => {
      // FIFO append; identical-message collapsing applies only within the 150ms
      // floor window of the immediately preceding entry (spec: announcer).
      const last = queue[queue.length - 1];
      const collapsed = politeness === "polite" && last?.text === text && at - last.at < FLOOR_MS;
      return collapsed ? queue : [...queue, { id, text, at }];
    });
    const timer = window.setTimeout(() => {
      setter((queue) => queue.filter((e) => e.id !== id));
    }, FLOOR_MS);
    timersRef.current.push(timer);
  }, []);

  const value = useMemo(() => ({ announce }), [announce]);

  return (
    <AnnouncerContext.Provider value={value}>
      {children}
      <div aria-live="polite" aria-atomic="true" className="a11y-announcer">
        {polite.map((e) => (
          <p key={e.id}>{e.text}</p>
        ))}
      </div>
      <div role="alert" aria-live="assertive" aria-atomic="true" className="a11y-announcer">
        {assertive.map((e) => (
          <p key={e.id}>{e.text}</p>
        ))}
      </div>
    </AnnouncerContext.Provider>
  );
}
