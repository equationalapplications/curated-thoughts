import { screen, fireEvent, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { TimelineMode } from "../components/modes/TimelineMode";
import { renderWithTheme } from "./test-utils";

const SAMPLE_EVENTS = [
  { id: "1", kind: "approved", summary: "Approved fact", entity_id: null, entity_name: null, doc_path: null, raw_type: "approved", client: null, created_at_ms: 1000 },
  { id: "2", kind: "agent_access", summary: "agent called tool", entity_id: null, entity_name: null, doc_path: null, raw_type: "tool", client: "test", created_at_ms: 2000 },
  { id: "3", kind: "ingested", summary: "Ingested *doc.md*", entity_id: null, entity_name: null, doc_path: "doc.md", raw_type: "indexed", client: null, created_at_ms: 3000 },
];

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "list_events") return Promise.resolve(SAMPLE_EVENTS);
    return Promise.resolve(null);
  });
});

test("renders all events initially", async () => {
  renderWithTheme(<TimelineMode />);
  await waitFor(() => {
    expect(screen.getByText("Approved fact")).toBeInTheDocument();
    expect(screen.getByText("agent called tool")).toBeInTheDocument();
    expect(screen.getByText("Ingested *doc.md*")).toBeInTheDocument();
  });
});

test("filters by kind", async () => {
  renderWithTheme(<TimelineMode />);
  await waitFor(() => {
    expect(screen.getByText("Approved fact")).toBeInTheDocument();
  });

  // Click the "approved" checkbox to deselect it
  fireEvent.click(screen.getByLabelText("approved"));
  // Now only agent_access and ingested should remain
  await waitFor(() => {
    expect(screen.queryByText("Approved fact")).not.toBeInTheDocument();
    expect(screen.getByText("agent called tool")).toBeInTheDocument();
    expect(screen.getByText("Ingested *doc.md*")).toBeInTheDocument();
  });
});

test("filters by entity name", async () => {
  renderWithTheme(<TimelineMode />);
  await waitFor(() => {
    expect(screen.getByText("Approved fact")).toBeInTheDocument();
  });

  const input = screen.getByPlaceholderText("Filter by entity…");
  fireEvent.change(input, { target: { value: "test" } });

  await waitFor(() => {
    expect(screen.getByText("agent called tool")).toBeInTheDocument();
    expect(screen.queryByText("Approved fact")).not.toBeInTheDocument();
  });
});

test("clear filters resets all filters", async () => {
  renderWithTheme(<TimelineMode />);
  await waitFor(() => {
    expect(screen.getByText("Approved fact")).toBeInTheDocument();
  });

  // Select a kind filter
  fireEvent.click(screen.getByLabelText("approved"));
  // Now only agent_access and ingested should be visible
  await waitFor(() => {
    expect(screen.queryByText("Approved fact")).not.toBeInTheDocument();
  });

  // Click clear filters
  fireEvent.click(screen.getByText("Clear filters"));
  await waitFor(() => {
    expect(screen.getByText("Approved fact")).toBeInTheDocument();
    expect(screen.getByText("agent called tool")).toBeInTheDocument();
    expect(screen.getByText("Ingested *doc.md*")).toBeInTheDocument();
  });
});

test("date filter narrows events", async () => {
  renderWithTheme(<TimelineMode />);
  await waitFor(() => {
    expect(screen.getByText("Approved fact")).toBeInTheDocument();
  });

  // Set "From" date to after the first event (created_at_ms 1000)
  const fromInput = screen.getAllByPlaceholderText("From")[0];
  fireEvent.change(fromInput, { target: { value: "1970-01-02" } }); // after 1000ms

  await waitFor(() => {
    // Event 1 (1000ms) should be filtered out
    expect(screen.queryByText("Approved fact")).not.toBeInTheDocument();
    // Events 2 and 3 should remain
    expect(screen.getByText("agent called tool")).toBeInTheDocument();
    expect(screen.getByText("Ingested *doc.md*")).toBeInTheDocument();
  });
});
