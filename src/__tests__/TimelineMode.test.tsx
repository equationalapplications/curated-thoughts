import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { TimelineMode } from "../components/modes/TimelineMode";
import type { TimelineEvent } from "../lib/tauri";

vi.mock("@tauri-apps/api/core");

const SAMPLE_EVENTS: TimelineEvent[] = [
  {
    id: "evt-1",
    kind: "synthesized",
    summary: "Created *Entity Alpha*",
    entity_id: "entity-1",
    entity_name: "Entity Alpha",
    created_at_ms: Date.now(),
    raw_type: "entity.created",
    client: "web",
  },
  {
    id: "evt-2",
    kind: "approved",
    summary: "Approved changes",
    entity_id: "entity-2",
    entity_name: "Entity Beta",
    created_at_ms: Date.now() - 1000,
    raw_type: "proposal.approved",
  },
  {
    id: "evt-3",
    kind: "ingested",
    summary: "Imported document",
    doc_path: "documents/sample.md",
    created_at_ms: Date.now() - 2000,
    raw_type: "doc.ingested",
  },
];

beforeEach(() => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "list_events_cmd") {
      return Promise.resolve(SAMPLE_EVENTS);
    }
    return Promise.resolve(null);
  });
});

describe("TimelineMode", () => {
  it("renders_day_grouped_events_with_kind_icons", async () => {
    render(<TimelineMode onNavigate={vi.fn()} />);

    // Wait for events to load
    await waitFor(() => {
      expect(screen.queryByText(/Created/)).toBeInTheDocument();
    });

    // Check that icons are rendered (emoji content)
    expect(screen.getByText("✨")).toBeInTheDocument(); // synthesized icon
    expect(screen.getByText("✅")).toBeInTheDocument(); // approved icon
    expect(screen.getByText("📄")).toBeInTheDocument(); // ingested icon
  });

  it("clicking_entity_event_navigates_to_brain", async () => {
    const onNavigate = vi.fn();
    render(<TimelineMode onNavigate={onNavigate} />);

    await waitFor(() => {
      expect(screen.queryByText(/Created/)).toBeInTheDocument();
    });

    // Click on an entity event
    const entityEvent = screen.getByText(/Created/).closest(".event-row");
    if (entityEvent) {
      fireEvent.click(entityEvent);
    }

    expect(onNavigate).toHaveBeenCalledWith({
      mode: "brain",
      entityId: "entity-1",
    });
  });

  it("clicking_doc_event_navigates_to_library", async () => {
    const onNavigate = vi.fn();
    render(<TimelineMode onNavigate={onNavigate} />);

    await waitFor(() => {
      expect(screen.queryByText(/Imported document/)).toBeInTheDocument();
    });

    // Click on a doc event
    const docEvent = screen.getByText(/Imported document/).closest(".event-row");
    if (docEvent) {
      fireEvent.click(docEvent);
    }

    expect(onNavigate).toHaveBeenCalledWith({
      mode: "library",
      docPath: "documents/sample.md",
    });
  });

  it("power_layer_toggle_reveals_raw_type_and_client", async () => {
    render(<TimelineMode onNavigate={vi.fn()} />);

    await waitFor(() => {
      expect(screen.queryByText(/Created/)).toBeInTheDocument();
    });

    // Power layer should not be visible initially
    expect(screen.queryByText(/entity\.created/)).not.toBeInTheDocument();

    // Toggle power layer
    const powerLayerCheckbox = screen.getByRole("checkbox", {
      name: /Power layer/,
    });
    fireEvent.click(powerLayerCheckbox);

    // Now power layer should be visible
    await waitFor(() => {
      expect(screen.getByText(/entity\.created/)).toBeInTheDocument();
    });
  });

  it("kind_filter_narrows_listEvents_call", async () => {
    render(<TimelineMode onNavigate={vi.fn()} />);

    await waitFor(() => {
      expect(screen.queryByText(/Created/)).toBeInTheDocument();
    });

    // Get the synthesized kind checkbox
    const synthesizedCheckbox = screen.getByRole("checkbox", {
      name: /Synthesized/,
    });

    // Click it to enable the filter
    fireEvent.click(synthesizedCheckbox);

    // Verify the hook was called with the kinds filter
    await waitFor(() => {
      const lastCall = vi.mocked(invoke).mock.lastCall;
      expect(lastCall?.[0]).toBe("list_events_cmd");
      const filter = lastCall?.[1]?.filter;
      expect(filter?.kinds).toContain("synthesized");
    });
  });

  it("entity_filter_narrows_displayed_events_client_side", async () => {
    render(<TimelineMode onNavigate={vi.fn()} />);

    await waitFor(() => {
      expect(screen.queryByText(/Created/)).toBeInTheDocument();
    });

    // Get the entity filter input
    const entityInput = screen.getByPlaceholderText(/Filter by entity name/);

    // Filter for "Alpha"
    fireEvent.change(entityInput, { target: { value: "Alpha" } });

    // Should only see Entity Alpha event
    await waitFor(() => {
      expect(screen.getByText(/Created/)).toBeInTheDocument();
    });

    // Beta event should not be visible (filtered out)
    expect(screen.queryByText(/Approved changes/)).not.toBeInTheDocument();
  });

  it("clear_filters_button_resets_all_filters", async () => {
    render(<TimelineMode onNavigate={vi.fn()} />);

    await waitFor(() => {
      expect(screen.queryByText(/Created/)).toBeInTheDocument();
    });

    // Enable some filters
    const synthesizedCheckbox = screen.getByRole("checkbox", {
      name: /Synthesized/,
    });
    fireEvent.click(synthesizedCheckbox);

    const entityInput = screen.getByPlaceholderText(/Filter by entity name/);
    fireEvent.change(entityInput, { target: { value: "Alpha" } });

    // Clear filters button should appear
    await waitFor(() => {
      expect(screen.getByText(/Clear filters/)).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText(/Clear filters/));

    // Checkbox should be unchecked
    expect(synthesizedCheckbox).not.toBeChecked();

    // Entity input should be empty
    expect(entityInput).toHaveValue("");
  });
});
