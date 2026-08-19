import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { vi } from "vitest";

vi.mock("@tauri-apps/api/core");

vi.mock("../components/brain/EntitySummarySection", () => ({
  EntitySummarySection: ({ summary }: { summary: string }) => (
    <div data-testid="summary-section">{summary}</div>
  ),
}));

import { EntityPage } from "../components/brain/EntityPage";
import type { EntityDetail } from "../lib/tauri";

const DETAIL: EntityDetail = {
  id: "ent_1",
  name: "Project X",
  entity_type: "project",
  summary: "The flagship.",
  created_at: 1750000000,
  updated_at: 1750086400,
  deleted_at: null,
  facts: [
    {
      id: "fact_1",
      title: "Ships Fridays",
      body: "Ships Fridays.",
      tags: [],
      confidence: "confirmed",
      source_type: "user_stated",
      source_docs: [],
      updated_at: 1750000000000,
    },
  ],
  tasks: [
    {
      id: "task_1",
      description: "Verify launch date",
      status: "pending",
      priority: 0,
      created_at: 1750000000000,
    },
  ],
  events: [
    {
      id: "evt_1",
      event_type: "action",
      summary: "Approved proposal for *Project X*",
      related_entry_id: null,
      created_at: 1750000000000,
    },
  ],
};

function mockDetail(detail: EntityDetail | null) {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "get_entity_cmd") return Promise.resolve(detail);
    if (cmd === "add_entity_fact_cmd") return Promise.resolve(null);
    return Promise.resolve(null);
  });
}

const NOOPS = {
  onNavigateEntity: vi.fn(),
  onOpenSource: vi.fn(),
  onEntityLoaded: vi.fn(),
  onMutated: vi.fn(),
  onArchived: vi.fn(),
};

test("shows empty state without a selection", () => {
  render(<EntityPage entityId={null} {...NOOPS} />);
  expect(screen.getByText(/No entity selected/)).toBeInTheDocument();
});

test("renders header, facts, tasks, and events", async () => {
  mockDetail(DETAIL);
  render(<EntityPage entityId="ent_1" {...NOOPS} />);
  expect(await screen.findByRole("heading", { name: "Project X" })).toBeInTheDocument();
  expect(screen.getByText("project")).toBeInTheDocument();
  expect(screen.getByText(/1 fact/)).toBeInTheDocument();
  expect(screen.getByText("Ships Fridays.")).toBeInTheDocument();
  expect(screen.getByText("Verify launch date")).toBeInTheDocument();
  expect(screen.getByText(/Approved proposal/)).toBeInTheDocument();
});

test("add fact form submits and reloads", async () => {
  mockDetail(DETAIL);
  const onMutated = vi.fn();
  render(<EntityPage entityId="ent_1" {...NOOPS} onMutated={onMutated} />);
  await screen.findByRole("heading", { name: "Project X" });

  fireEvent.change(screen.getByPlaceholderText("Add a fact..."), {
    target: { value: "New fact." },
  });
  fireEvent.click(screen.getByRole("button", { name: "Add fact" }));

  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("add_entity_fact_cmd", {
      entityId: "ent_1",
      body: "New fact.",
    }),
  );
  await waitFor(() => expect(onMutated).toHaveBeenCalled());
});
