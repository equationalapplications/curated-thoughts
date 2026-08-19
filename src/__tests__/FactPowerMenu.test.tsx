import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FactPowerMenu } from "../components/brain/FactPowerMenu";
import type { EntityFact } from "../lib/tauri";

const baseFact: EntityFact = {
  id: "fact_x",
  title: "T",
  body: "B",
  tags: [],
  confidence: "certain",
  source_type: "user_stated",
  source_docs: [],
  updated_at: 1700000000000,
  lifecycle_status: "stable",
  stale_after: null,
  generated_by: "human:alice",
  okf_sources: [{ resource: "documents/notes.md", usage_count: 3 }],
  okf_verified: [{ by: "process:nightly", at: 1700100000000 }],
  okf_usage_window: { from: "2026-07-01", to: "2026-12-31" },
  last_verified_at: 1700100000000,
  last_verified_by: "process:nightly",
};

describe("FactPowerMenu", () => {
  it("renders raw id, provenance, lifecycle, and usage window", () => {
    render(<FactPowerMenu fact={baseFact} open onClose={vi.fn()} />);
    expect(screen.getByText("fact_x")).toBeInTheDocument();
    expect(screen.getByText("human:alice")).toBeInTheDocument();
    expect(screen.getByText("process:nightly")).toBeInTheDocument();
    expect(screen.getByText("documents/notes.md")).toBeInTheDocument();
    expect(screen.getByText(/2026-07-01.*2026-12-31/)).toBeInTheDocument();
    expect(screen.getByText("stable")).toBeInTheDocument();
  });

  it("renders empty state when okf_sources is empty", () => {
    render(<FactPowerMenu fact={{ ...baseFact, okf_sources: [], okf_verified: [] }} open onClose={vi.fn()} />);
    expect(screen.getByText(/no provenance recorded/i)).toBeInTheDocument();
  });

  it("calls onClose on Escape", async () => {
    const onClose = vi.fn();
    render(<FactPowerMenu fact={baseFact} open onClose={onClose} />);
    await userEvent.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });
});