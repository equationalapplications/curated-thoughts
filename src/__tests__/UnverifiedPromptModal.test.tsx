import {
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import {
  setSnoozedUntil,
  snoozedUntil,
  UnverifiedPromptModal,
} from "../components/queue/UnverifiedPromptModal";
import { renderWithTheme } from "./test-utils";
import type { QueueRow } from "../lib/tauri";

const REAL_ROW: QueueRow = {
  fact_id: "fact_tampered",
  title: "Real title from the brain",
  body: "Real body from the brain.",
  provenance: "librarian_inferred",
  entity_id: "ent_1",
  evidence_quote: "Real evidence from the source document.",
  source_document: "documents/real.md",
  attributed_speaker: null,
  instruction_source: null,
  age_days: 1,
};

function defaultInvoke(cmd: string) {
  // THE PAYLOAD-TAMPERING SCENARIO (spec §4 "Modal abuse"): the drop-file
  // line carries poisoned display text. The backend drain deliberately
  // forwards ONLY fact_id + ts — but this test proves the contract end to
  // end: even if the payload rode along, the modal renders the BRAIN row.
  if (cmd === "unverified_prompts_drain_cmd") {
    return Promise.resolve({
      prompts: [
        {
          fact_id: "fact_tampered",
          ts: 1,
          title: "TAMPERED PAYLOAD TITLE",
          body: "TAMPERED PAYLOAD BODY",
          quote: "TAMPERED PAYLOAD QUOTE",
        },
      ],
      total_unacked: 4,
    });
  }
  if (cmd === "queue_fact_get_cmd") return Promise.resolve(REAL_ROW);
  if (cmd === "queue_confirm_cmd") return Promise.resolve();
  if (cmd === "queue_deny_cmd") return Promise.resolve();
  return Promise.resolve(null);
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(defaultInvoke as never);
  window.localStorage.clear();
});

test("modal shows the brain content, NEVER the drop-file payload text", async () => {
  renderWithTheme(<UnverifiedPromptModal pollMs={3600_000} />);
  expect(
    await screen.findByText("Real body from the brain."),
  ).toBeInTheDocument();
  expect(screen.getByText("Real title from the brain")).toBeInTheDocument();
  expect(
    screen.getByText(/real evidence from the source document\./i),
  ).toBeInTheDocument();
  for (const poisoned of [
    /TAMPERED PAYLOAD TITLE/,
    /TAMPERED PAYLOAD BODY/,
    /TAMPERED PAYLOAD QUOTE/,
  ]) {
    expect(screen.queryByText(poisoned)).not.toBeInTheDocument();
  }
  // The poisoned text must not leak through any rendered text either.
  const modal = screen.getByRole("dialog");
  expect(modal.textContent).not.toContain("TAMPERED PAYLOAD");
});

test("per-drain digest shows the remaining unacked count", async () => {
  renderWithTheme(<UnverifiedPromptModal pollMs={3600_000} />);
  await screen.findByText("Real body from the brain.");
  expect(screen.getByText(/and 4 more/i)).toBeInTheDocument();
});

test("confirm from the prompt invokes the queue confirm", async () => {
  renderWithTheme(<UnverifiedPromptModal pollMs={3600_000} />);
  await screen.findByText("Real body from the brain.");
  fireEvent.click(screen.getByRole("button", { name: /^confirm$/i }));
  await waitFor(() => {
    expect(invoke).toHaveBeenCalledWith("queue_confirm_cmd", {
      factId: "fact_tampered",
    });
  });
});

test("snooze persists the preference and closes the modal", async () => {
  renderWithTheme(<UnverifiedPromptModal pollMs={3600_000} />);
  await screen.findByText("Real body from the brain.");
  fireEvent.click(screen.getByRole("button", { name: /snooze for a week/i }));
  await waitFor(() => {
    expect(snoozedUntil()).toBeGreaterThan(Date.now());
  });
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});

test("a snoozed modal never drains", async () => {
  setSnoozedUntil(Date.now() + 60_000);
  renderWithTheme(<UnverifiedPromptModal pollMs={3600_000} />);
  await waitFor(() => {
    expect(invoke).not.toHaveBeenCalledWith(
      "unverified_prompts_drain_cmd",
    );
  });
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});

// --- R3b: focus containment & restore (a11y dialog pattern) ---

function makeRow(id: string, title: string, body: string): QueueRow {
  return {
    fact_id: id,
    title,
    body,
    provenance: "librarian_inferred",
    entity_id: "ent_1",
    evidence_quote: null,
    source_document: "documents/real.md",
    attributed_speaker: null,
    instruction_source: null,
    age_days: 1,
  };
}

test("focus moves into the dialog on open (first action button focused)", async () => {
  renderWithTheme(<UnverifiedPromptModal pollMs={3600_000} />);
  await screen.findByText("Real body from the brain.");
  expect(document.activeElement).toBe(
    screen.getByRole("button", { name: /^confirm$/i }),
  );
});

test("after confirming one of two facts, focus is on a remaining action button (NOT document.body)", async () => {
  const rowA = makeRow("fact_a", "Fact A title", "Fact A body.");
  const rowB = makeRow("fact_b", "Fact B title", "Fact B body.");
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(((cmd: string, args?: { factId?: string }) => {
    if (cmd === "unverified_prompts_drain_cmd") {
      return Promise.resolve({
        prompts: [
          { fact_id: "fact_a", ts: 1 },
          { fact_id: "fact_b", ts: 2 },
        ],
        total_unacked: 2,
      });
    }
    if (cmd === "queue_fact_get_cmd") {
      return Promise.resolve(args?.factId === "fact_a" ? rowA : rowB);
    }
    if (cmd === "queue_confirm_cmd") return Promise.resolve();
    if (cmd === "queue_deny_cmd") return Promise.resolve();
    return Promise.resolve(null);
  }) as never);
  renderWithTheme(<UnverifiedPromptModal pollMs={3600_000} />);
  await screen.findByText("Fact A title");
  const confirms = screen.getAllByRole("button", { name: /^confirm$/i });
  expect(confirms).toHaveLength(2);
  const confirmB = confirms[1];
  fireEvent.click(confirms[0]);
  await waitFor(() => {
    expect(invoke).toHaveBeenCalledWith("queue_confirm_cmd", {
      factId: "fact_a",
    });
  });
  // Focus handoff lands after React commits the removal, so wait for it.
  await waitFor(() => {
    expect(document.activeElement).toBe(confirmB);
  });
  expect(document.activeElement).not.toBe(document.body);
});

test("Tab from the last focusable wraps to the first focusable inside the dialog", async () => {
  renderWithTheme(<UnverifiedPromptModal pollMs={3600_000} />);
  await screen.findByText("Real body from the brain.");
  const dialog = screen.getByRole("dialog");
  const buttons = within(dialog).getAllByRole("button");
  const last = buttons[buttons.length - 1];
  expect(last).toHaveTextContent(/not now/i);
  last.focus();
  expect(document.activeElement).toBe(last);
  fireEvent.keyDown(last, { key: "Tab" });
  expect(document.activeElement).toBe(
    screen.getByRole("button", { name: /^confirm$/i }),
  );
});

test("on close, focus returns to the previously focused element", async () => {
  const { container } = renderWithTheme(
    <>
      <button type="button" data-testid="opener">
        Something that had focus
      </button>
      <UnverifiedPromptModal pollMs={3600_000} />
    </>,
  );
  const opener = container.querySelector(
    "[data-testid=opener]",
  ) as HTMLButtonElement;
  opener.focus();
  await screen.findByText("Real body from the brain.");
  fireEvent.click(screen.getByRole("button", { name: /not now/i }));
  await waitFor(() => {
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  expect(document.activeElement).toBe(opener);
});
