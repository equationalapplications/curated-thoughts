import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ReviewEvidencePanel } from "../components/review/ReviewEvidencePanel";
import { makeProposalSummary } from "./fixtures/proposals";

const PROPOSAL = makeProposalSummary({
  id: "prop_1",
  target_name: "Project X",
  created_at: 1,
  source_doc_paths: ["documents/notes.md", "documents/meeting.pdf"],
});

describe("ReviewEvidencePanel", () => {
  it("renders source doc names from source_doc_paths", () => {
    render(<ReviewEvidencePanel proposal={PROPOSAL} />);

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
    const empty = makeProposalSummary({
      id: "prop_empty",
      target_name: "Empty",
      created_at: 1,
      source_doc_paths: [],
    });

    render(<ReviewEvidencePanel proposal={empty} />);
    expect(screen.getByText(/no source documents cited/i)).toBeInTheDocument();
  });

  it("shows chunk placeholder when chunks are unavailable", () => {
    render(<ReviewEvidencePanel proposal={PROPOSAL} />);
    expect(
      screen.getByText(/source chunks not available/i),
    ).toBeInTheDocument();
  });

  it('shows "Not recorded" when reasoning is missing', () => {
    render(<ReviewEvidencePanel proposal={PROPOSAL} />);
    expect(screen.getByText("Not recorded")).toBeInTheDocument();
  });

  it("renders reasoning when provided", () => {
    render(
      <ReviewEvidencePanel
        proposal={PROPOSAL}
        reasoning="Meeting notes mention a Q3 budget reallocation to Project X."
      />,
    );
    expect(
      screen.getByText(/budget reallocation to Project X/i),
    ).toBeInTheDocument();
  });

  it("calls onSourceClick with full path when a source is clicked", () => {
    const onSourceClick = vi.fn();
    render(
      <ReviewEvidencePanel proposal={PROPOSAL} onSourceClick={onSourceClick} />,
    );

    fireEvent.click(screen.getByRole("button", { name: "notes.md" }));
    expect(onSourceClick).toHaveBeenCalledWith("documents/notes.md");
  });
});
