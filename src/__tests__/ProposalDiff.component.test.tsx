import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { ProposalDiff } from "../components/review/ProposalDiff";
import {
  MODERATE_REWRITE_NEW,
  MODERATE_REWRITE_OLD,
  PARAGRAPH_REWRITE_NEW,
  PARAGRAPH_REWRITE_OLD,
} from "./fixtures/paragraph-rewrite";

describe("ProposalDiff", () => {
  it("renders inline word-level diff for moderate changes", () => {
    render(
      <ProposalDiff
        oldText={MODERATE_REWRITE_OLD}
        newText={MODERATE_REWRITE_NEW}
      />,
    );

    const diff = screen.getByTestId("proposal-diff");
    expect(diff).toHaveAttribute("data-mode", "inline");
    expect(diff.querySelector(".proposal-diff-added")).toBeTruthy();
  });

  it("renders side-by-side panes for high-churn changes", () => {
    render(
      <ProposalDiff
        oldText={PARAGRAPH_REWRITE_OLD}
        newText={PARAGRAPH_REWRITE_NEW}
      />,
    );

    const diff = screen.getByTestId("proposal-diff");
    expect(diff).toHaveAttribute("data-mode", "side-by-side");
    expect(screen.getByText("Current")).toBeInTheDocument();
    expect(screen.getByText("Proposed")).toBeInTheDocument();
    expect(screen.getByText(PARAGRAPH_REWRITE_OLD)).toBeInTheDocument();
    expect(screen.getByText(PARAGRAPH_REWRITE_NEW)).toBeInTheDocument();
  });
});
