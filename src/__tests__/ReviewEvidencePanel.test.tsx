import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ReviewEvidencePanel } from "../components/review/ReviewEvidencePanel";
import type { ReviewPage } from "../lib/tauri";

const PAGE = {
  id: 1,
  path: "wiki/Project-X.md",
  generated_by: "llama3.2:3b",
  source_doc_ids: '["documents/notes.md", "documents/meeting.pdf"]',
} as unknown as ReviewPage;

describe("ReviewEvidencePanel", () => {
  it("renders source doc names from source_doc_ids", () => {
    render(<ReviewEvidencePanel page={PAGE} />);

    expect(screen.getByRole("button", { name: "notes.md" })).toHaveAttribute(
      "title",
      "documents/notes.md",
    );
    expect(screen.getByRole("button", { name: "meeting.pdf" })).toHaveAttribute(
      "title",
      "documents/meeting.pdf",
    );
  });

  it("shows placeholder when no source documents are cited", () => {
    const empty = {
      ...PAGE,
      source_doc_ids: "[]",
    } as unknown as ReviewPage;

    render(<ReviewEvidencePanel page={empty} />);
    expect(screen.getByText(/no source documents cited/i)).toBeInTheDocument();
  });

  it("shows chunk placeholder when chunks are unavailable", () => {
    render(<ReviewEvidencePanel page={PAGE} />);
    expect(
      screen.getByText(/source chunks not available/i),
    ).toBeInTheDocument();
  });

  it('shows "Not recorded" when reasoning summary is missing', () => {
    render(<ReviewEvidencePanel page={PAGE} />);
    expect(screen.getByText("Not recorded")).toBeInTheDocument();
  });

  it("renders reasoning summary when provided", () => {
    const withReasoning = {
      ...PAGE,
      reasoning_summary:
        "Meeting notes mention a Q3 budget reallocation to Project X.",
    } as unknown as ReviewPage;

    render(<ReviewEvidencePanel page={withReasoning} />);
    expect(
      screen.getByText(/budget reallocation to Project X/i),
    ).toBeInTheDocument();
  });

  it("calls onSourceClick with full path when a source is clicked", () => {
    const onSourceClick = vi.fn();
    render(<ReviewEvidencePanel page={PAGE} onSourceClick={onSourceClick} />);

    fireEvent.click(screen.getByRole("button", { name: "notes.md" }));
    expect(onSourceClick).toHaveBeenCalledWith("documents/notes.md");
  });
});
