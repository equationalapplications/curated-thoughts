import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ReviewEvidencePanel } from "../components/review/ReviewEvidencePanel";
import type { ProposalItem } from "../lib/tauri";
import { makeProposalSummary } from "./fixtures/proposals";

const PROPOSAL = makeProposalSummary({
  id: "prop_1",
  target_name: "Project X",
  created_at: 1,
  source_doc_paths: ["documents/notes.md", "documents/meeting.pdf"],
});

const EVIDENCE_ITEMS: ProposalItem[] = [
  {
    id: "item_1",
    item_type: "fact_add",
    target_id: null,
    payload: { body: "Fact from source." },
    evidence: [
      {
        chunk_id: 1,
        quote: "Budget increased in Q3 by 10%.",
        start_line: 12,
        end_line: 12,
        doc_path: "documents/notes.md",
        source_deleted: false,
      },
    ],
    status: "pending",
    edited_payload: null,
  },
];

describe("ReviewEvidencePanel", () => {
  it("renders source doc names from source_doc_paths", () => {
    render(<ReviewEvidencePanel proposal={PROPOSAL} items={EVIDENCE_ITEMS} />);

    expect(screen.getByRole("button", { name: "notes.md" })).toHaveAttribute(
      "title",
      "documents/notes.md",
    );
    expect(screen.getByRole("button", { name: "meeting.pdf" })).toHaveAttribute(
      "title",
      "documents/meeting.pdf",
    );
  });

  it("marks a stranded proposal and names its deleted sources", () => {
    const stranded = makeProposalSummary({
      id: "prop_stranded",
      target_name: "Stranded",
      created_at: 1,
      source_doc_paths: [],
      deleted_source_paths: ["documents/gone.md"],
    });

    render(<ReviewEvidencePanel proposal={stranded} />);

    expect(
      screen.getByText(
        "All sources deleted — approving will skip facts unless a source returns at its original path.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("Source deleted: gone.md")).toHaveAttribute(
      "title",
      "documents/gone.md",
    );
    expect(screen.queryByRole("button", { name: /gone\.md/ })).toBeNull();
    expect(screen.queryByText(/no source documents cited/i)).toBeNull();
  });

  it("marks a proposal stranded before deleted sources were recorded", () => {
    const legacy = makeProposalSummary({
      id: "prop_legacy",
      target_name: "Legacy",
      created_at: 1,
      source_doc_paths: [],
      deleted_source_paths: [],
    });

    render(<ReviewEvidencePanel proposal={legacy} />);

    expect(screen.getByText(/^All sources deleted/)).toBeInTheDocument();
    expect(screen.queryByText(/no source documents cited/i)).toBeNull();
  });

  it("lists a deleted source beside live ones without the stranded marker", () => {
    const partial = makeProposalSummary({
      id: "prop_partial",
      target_name: "Partial",
      created_at: 1,
      source_doc_paths: ["documents/notes.md"],
      deleted_source_paths: ["documents/old.md"],
    });

    render(<ReviewEvidencePanel proposal={partial} />);

    expect(screen.getByRole("button", { name: "notes.md" })).toBeInTheDocument();
    expect(screen.getByText("Source deleted: old.md")).toBeInTheDocument();
    expect(screen.queryByText(/^All sources deleted/)).toBeNull();
  });

  it("shows chunk placeholder when chunks are unavailable", () => {
    render(<ReviewEvidencePanel proposal={PROPOSAL} />);
    expect(
      screen.getByText(/source chunks not available/i),
    ).toBeInTheDocument();
  });

  it("renders chunk quotes and line ranges when available", () => {
    render(<ReviewEvidencePanel proposal={PROPOSAL} items={EVIDENCE_ITEMS} />);
    expect(
      screen.getByText(/Budget increased in Q3 by 10%\./i),
    ).toBeInTheDocument();
    expect(screen.getByText(/notes\.md · L12/i)).toBeInTheDocument();
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
