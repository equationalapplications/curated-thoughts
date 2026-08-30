import { render, screen, fireEvent } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { ConnectionsPanel } from "../components/brain/ConnectionsPanel";
import { describe, test, expect, beforeEach, vi } from "vitest";

const CONNECTIONS = {
  outgoing: [
    {
      id: "edge_1",
      edge_type: "blocks",
      source_id: "fact_a",
      source_label: "Fact A",
      target_id: "task_b",
      target_label: "Task B",
    },
    {
      id: "edge_2",
      edge_type: "relates_to",
      source_id: "fact_a",
      source_label: "Fact A",
      target_id: "fact_c",
      target_label: "Fact C",
    },
  ],
  backlinks: [{ entity_id: "ent_9", name: "Beta Team", entity_type: "team" }],
};

describe("ConnectionsPanel", () => {
  beforeEach(() => {
    // Override only the connections query and fall through to the test-setup
    // default for everything else. Critically, get_provider_config must keep
    // returning a valid config so useProviderHealth resolves embedding to
    // "ok" — otherwise the panel renders only the ProviderNotice, the
    // backlinks never appear, and the click assertion below race-flakes on
    // microtask ordering of the getEntityConnections vs getProviderConfig
    // .then arms.
    const fallback = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "get_entity_connections_cmd") return Promise.resolve(CONNECTIONS);
      return fallback!(cmd, args);
    });
  });

  test("renders backlinks and edges grouped by type; backlink click selects entity", async () => {
    const onSelectEntity = vi.fn();
    render(<ConnectionsPanel entityId="ent_1" onSelectEntity={onSelectEntity} />);

    fireEvent.click(await screen.findByRole("button", { name: "Beta Team" }));
    expect(onSelectEntity).toHaveBeenCalledWith("ent_9");

    expect(screen.getByRole("heading", { name: "blocks" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "relates_to" })).toBeInTheDocument();
    expect(screen.getByText("Fact A → Task B")).toBeInTheDocument();
  });

  test("renders nothing without a selection", () => {
    const { container } = render(
      <ConnectionsPanel entityId={null} onSelectEntity={vi.fn()} />,
    );
    expect(container.querySelector(".connections-panel")).toBeNull();
  });
});
