// D6b regression tests: the queue rail badge must never display a false zero
// when a `queue_list` fetch fails. Failure keeps the last known count; when
// nothing is known yet, the badge shows an explicit unknown state ("?").
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, cleanup, screen } from "@testing-library/react";

// Explicit resolving stubs for exactly what AppShell + its hooks call at
// mount (AppShell.dragdrop.test.tsx precedent — never a Proxy). listQueue is
// overridden per-test; everything else stays a quiet resolve.
vi.mock("../lib/tauri", () => ({
  needsChunkHashMigration: vi.fn(() => Promise.resolve(false)),
  startFileWatcher: vi.fn(() => Promise.resolve(() => {})),
  peekPendingConfigMalformed: vi.fn(() => Promise.resolve(null)),
  ackPendingConfigMalformed: vi.fn(() => Promise.resolve()),
  listProposals: vi.fn(() => Promise.resolve([])),
  listQueue: vi.fn(() => Promise.resolve([])),
  listDenied: vi.fn(() => Promise.resolve([])),
  confirmQueueFact: vi.fn(() => Promise.resolve()),
  denyQueueFact: vi.fn(() => Promise.resolve()),
  unDenyQueueFact: vi.fn(() => Promise.resolve()),
  captureStatement: vi.fn(() => Promise.resolve("stated/kurt/x.md")),
  confirmOrCapture: vi.fn(() => Promise.resolve("confirmed")),
  getQueueFact: vi.fn(() => Promise.resolve(null)),
  drainUnverifiedPrompts: vi.fn(() =>
    Promise.resolve({ prompts: [], total_unacked: 0 }),
  ),
  getPrivacyMode: vi.fn(() => Promise.resolve("standard")),
  listEntities: vi.fn(() => Promise.resolve([])),
  getProviderConfig: vi.fn(() =>
    Promise.resolve({
      provider: "ollama",
      model: "llama3",
      base_url: "http://localhost:11434",
      embedding_model: null,
    }),
  ),
  getIndexingStatus: vi.fn(() =>
    Promise.resolve({ state: "idle", pending: 0, processed: 0 }),
  ),
  getWikiStatus: vi.fn(() =>
    Promise.resolve({
      ingest: { state: "idle" },
      librarian: false,
      healing: false,
      pruning: false,
      forgetting: false,
    }),
  ),
  subscribeEntityStatus: vi.fn(() => Promise.resolve(() => {})),
}));

import { AppShell } from "../components/shell/AppShell";
import { renderWithTheme } from "./test-utils";
import { listQueue, type QueueRow } from "../lib/tauri";

const VAULT = "/Users/test/Curated-Thoughts";

function queueRow(factId: string): QueueRow {
  return {
    fact_id: factId,
    title: `Fact ${factId}`,
    body: "Some body text.",
    provenance: null,
    entity_id: "ent_test",
    evidence_quote: null,
    source_document: null,
    attributed_speaker: null,
    instruction_source: null,
    age_days: 0,
  };
}

function renderAppShell() {
  return renderWithTheme(
    <AppShell vaultPath={VAULT} onVaultChanged={vi.fn()} needsSetup={false} />,
  );
}

/** Flush pending state updates (mount refresh / interval tick) inside act. */
async function flush(ticks = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ticks);
  });
}

function queueRailButton() {
  return screen.getByRole("button", { name: "Queue" });
}

describe("queue badge fetch failure (D6b)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(listQueue).mockResolvedValue([]);
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("keeps the last known count when a refresh fetch fails", async () => {
    vi.mocked(listQueue).mockResolvedValueOnce([
      queueRow("f1"),
      queueRow("f2"),
    ]);
    renderAppShell();
    await flush();
    expect(queueRailButton()).toHaveTextContent("2");

    // The 60s interval refresh now fails: the badge must NOT drop to zero —
    // zero reads as "nothing pending", which is exactly the lie D6b pins.
    vi.mocked(listQueue).mockRejectedValueOnce(new Error("backend down"));
    await flush(60_000);

    expect(queueRailButton()).toHaveTextContent("2");
    expect(queueRailButton()).not.toHaveTextContent("0");
  });

  it("shows an explicit unknown badge when the first fetch fails", async () => {
    vi.mocked(listQueue).mockRejectedValue(new Error("backend down"));
    renderAppShell();
    await flush();

    // Nothing is known yet: the badge shows the unknown marker instead of
    // silently reading as "nothing pending".
    expect(queueRailButton()).toHaveTextContent("?");
    expect(queueRailButton()).not.toHaveTextContent(/^0$/);
  });
});
