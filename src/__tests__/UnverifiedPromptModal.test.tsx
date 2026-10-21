import { fireEvent, screen, waitFor } from "@testing-library/react";
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
