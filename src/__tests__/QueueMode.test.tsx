import { fireEvent, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { QueueMode } from "../components/modes/QueueMode";
import { renderWithTheme } from "./test-utils";

const ROWS = [
  {
    fact_id: "fact_old",
    title: "Old fact",
    body: "The oldest fact body.",
    provenance: "librarian_inferred",
    entity_id: "ent_1",
    evidence_quote: "…the oldest fact body…",
    source_document: "stated/kurt/2026/note.md",
    attributed_speaker: null,
    instruction_source: null,
    age_days: 12,
  },
  {
    fact_id: "fact_new",
    title: "New fact",
    body: "A newer fact body.",
    provenance: null,
    entity_id: "ent_1",
    evidence_quote: null,
    source_document: null,
    attributed_speaker: "kurt",
    instruction_source: null,
    age_days: 0,
  },
];

const DENIED = [
  {
    document_path: "documents/old.md",
    chunk_hash: "abc",
    fact_text_hash: "def",
    denied_by: "human:kurt",
    denied_at: "2026-10-01T00:00:00+00:00",
  },
];

function defaultInvoke(cmd: string, args?: Record<string, unknown>) {
  if (cmd === "queue_list_cmd") return Promise.resolve(ROWS);
  if (cmd === "queue_denied_cmd") return Promise.resolve(DENIED);
  if (cmd === "queue_confirm_cmd") return Promise.resolve();
  if (cmd === "queue_deny_cmd") return Promise.resolve();
  if (cmd === "queue_undeny_cmd") return Promise.resolve();
  if (cmd === "queue_capture_cmd")
    return Promise.resolve("stated/kurt/2026/x.md");
  void args;
  return Promise.resolve(null);
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(defaultInvoke);
});

test("renders rows with evidence quote, aging, and unconfirmed wording", async () => {
  renderWithTheme(<QueueMode />);
  expect(
    await screen.findByRole("heading", { name: /confirmation queue \(2\)/i }),
  ).toBeInTheDocument();
  expect(await screen.findByText("The oldest fact body.")).toBeInTheDocument();
  expect(screen.getByText(/12 days old/i)).toBeInTheDocument();
  expect(screen.getByText(/today/i)).toBeInTheDocument();
  // The queue word is "unconfirmed" — never "pending" (Rule 3).
  expect(screen.getAllByText(/librarian inferred/i).length).toBeGreaterThan(0);
  expect(screen.getByText("unclassified")).toBeInTheDocument();
  expect(screen.getByText(/…the oldest fact body…/)).toBeInTheDocument();
  expect(screen.getByText(/stated\/kurt\/2026\/note\.md/)).toBeInTheDocument();
  // queue word guard: the word "pending" must not label queue state.
  expect(screen.queryByText(/pending/i)).not.toBeInTheDocument();
});

test("bulk select + confirm selected confirms each checked fact", async () => {
  renderWithTheme(<QueueMode />);
  await screen.findByText("The oldest fact body.");
  fireEvent.click(screen.getByLabelText("Select Old fact"));
  fireEvent.click(screen.getByLabelText("Select New fact"));
  fireEvent.click(screen.getByRole("button", { name: /confirm selected/i }));
  await waitFor(() => {
    expect(invoke).toHaveBeenCalledWith("queue_confirm_cmd", {
      factId: "fact_old",
    });
    expect(invoke).toHaveBeenCalledWith("queue_confirm_cmd", {
      factId: "fact_new",
    });
  });
});

test("capture box routes a fresh statement through queue_capture_cmd", async () => {
  renderWithTheme(<QueueMode />);
  await screen.findByText("The oldest fact body.");
  fireEvent.change(screen.getByLabelText(/state something new/i), {
    target: { value: "Alpha ships on Fridays." },
  });
  fireEvent.click(
    screen.getByRole("button", { name: /capture statement/i }),
  );
  await waitFor(() => {
    expect(invoke).toHaveBeenCalledWith("queue_capture_cmd", {
      text: "Alpha ships on Fridays.",
    });
  });
  await screen.findByRole("status");
  expect(screen.getByRole("status").textContent).toMatch(/captured to/i);
});

test("denied section lists denials and undeny invokes the command", async () => {
  renderWithTheme(<QueueMode />);
  expect(await screen.findByText(/documents\/old\.md/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: /undeny/i }));
  await waitFor(() => {
    expect(invoke).toHaveBeenCalledWith("queue_undeny_cmd", {
      key: {
        document_path: "documents/old.md",
        chunk_hash: "abc",
        fact_text_hash: "def",
      },
    });
  });
});

test("empty queue shows the queue-clear empty state", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "queue_list_cmd") return Promise.resolve([]);
    if (cmd === "queue_denied_cmd") return Promise.resolve([]);
    return Promise.resolve(null);
  });
  renderWithTheme(<QueueMode />);
  expect(await screen.findByText(/queue clear/i)).toBeInTheDocument();
});
