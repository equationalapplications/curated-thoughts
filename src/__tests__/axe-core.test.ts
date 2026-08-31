// axe smoke test (Task 1, WCAG AA foundation).
// jsdom axe: color-contrast is disabled because jsdom has no computed colors;
// contrast is enforced by src/__tests__/a11y-contrast.test.ts instead (Task 2).
// A green run here is NOT a full conformance claim — see the manual checklist.
import { describe, expect, it } from "vitest";
import { createElement } from "react";
import { render } from "@testing-library/react";
import axe from "axe-core";
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

describe("a11y: axe-core smoke (jsdom)", () => {
  it("FactPowerMenu renders with zero axe violations", async () => {
    const { container } = render(
      createElement(FactPowerMenu, { fact: baseFact, open: true, onClose: () => {} }),
    );
    const results = await axe.run(container, {
      rules: { "color-contrast": { enabled: false } },
    });
    expect(results.violations).toEqual([]);
  });
});
