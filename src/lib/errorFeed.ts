//! Background error feed — accumulates errors from background operations
//! (synthesis failures, indexing errors, etc.) and exposes them to the UI
//! via a simple subscription pattern.

type Listener = () => void;

export interface BackgroundError {
  id: number;
  message: string;
  at: number;
  retry?: () => Promise<void>;
}

let nextId = 1;
let errors: BackgroundError[] = [];
const listeners = new Set<Listener>();

function emit() {
  for (const fn of listeners) {
    fn();
  }
}

export function subscribe(fn: Listener): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function getErrors(): BackgroundError[] {
  return errors;
}

export function reportBackgroundError(
  message: string,
  retry?: () => Promise<void>
): void {
  // Replace existing entry with same message to avoid duplicates
  const existing = errors.find((e) => e.message === message);
  if (existing) {
    errors = errors.map((e) =>
      e.id === existing.id
        ? { ...e, at: Date.now(), retry }
        : e
    );
  } else {
    errors = [...errors, { id: nextId++, message, at: Date.now(), retry }];
  }
  emit();
}

export function dismissError(id: number): void {
  errors = errors.filter((e) => e.id !== id);
  emit();
}

export function retryError(id: number): Promise<void> {
  const entry = errors.find((e) => e.id === id);
  if (!entry?.retry) return Promise.resolve();
  return entry.retry().then(
    () => {
      dismissError(id);
    },
    (err) => {
      // Re-throw so the caller can catch if needed
      throw err;
    }
  );
}
