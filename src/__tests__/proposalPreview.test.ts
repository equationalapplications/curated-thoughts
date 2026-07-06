import { describe, expect, it } from "vitest";
import { formatProposalPreview } from "../lib/proposalPreview";
import { makeProposalDetail, makeProposalSummary } from "./fixtures/proposals";

describe("formatProposalPreview", () => {
  it("renders target name, reasoning, and fact items", () => {
    const summary = makeProposalSummary({
      id: "prop_1",
      target_name: "Project X",
      created_at: 1,
    });
    const detail = makeProposalDetail(summary, {
      reasoning: "Notes mention a budget change.",
      items: [
        {
          id: "item_1",
          item_type: "fact_add",
          target_id: null,
          payload: { body: "Budget increased in Q3." },
          evidence: [
            {
              chunk_id: 1,
              quote: "Q3 budget was raised.",
              start_line: 4,
              end_line: 4,
              source_deleted: false,
            },
          ],
          status: "pending",
          edited_payload: null,
        },
      ],
    });

    const preview = formatProposalPreview(detail);
    expect(preview).toContain("# Project X");
    expect(preview).toContain("Notes mention a budget change.");
    expect(preview).toContain("**Add fact:** Budget increased in Q3.");
    expect(preview).toContain('"Q3 budget was raised."');
  });
});
