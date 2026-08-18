export interface BackgroundError {
  id: number;
  message: string;
  at: number; // timestamp
  retry?: () => Promise<void>;
}

type Listener = (errors: BackgroundError[]) => void;

let nextId = 1;
let errors: BackgroundError[] = [];
const listeners = new Set<Listener>();

function emit() {
  for (const l of listeners) l(errors);
}

export function reportBackgroundError(
  message: string,
  retry?: () => Promise<void>
): void {
  // Refresh existing entries with the same message instead of stacking
  // duplicates (e.g. on every vault switch, or on every poll).
  const existing = errors.find((e) => e.message === message);
  if (existing) {
    existing.at = Date.now();
    if (retry) existing.retry = retry;
    emit();
    return;
  }
  errors = [...errors, { id: nextId++, message, at: Date.now(), retry }];
  emit();
}

export function dismissError(id: number): void {
  errors = errors.filter((e) => e.id !== id);
  emit();
}

export async function retryError(id: number): Promise<void> {
  const entry = errors.find((e) => e.id === id);
  if (!entry?.retry) return;
  await entry.retry(); // throws if retry fails; entry stays
  dismissError(id); // only dismiss on success
}

export function subscribeErrors(l: Listener): () => void {
  listeners.add(l);
  l(errors); // immediate emit of current state
  return () => listeners.delete(l);
}

export function getErrorSnapshot(): BackgroundError[] {
  return errors;
}

// Test utility: reset the error feed state
export function __resetErrorFeed(): void {
  nextId = 1;
  errors = [];
  listeners.clear();
}
