// Expanded axe suite (Task 6, WCAG AA foundation): real composed surfaces
// rendered through the a11y primitives, following each component's existing
// __tests__ render pattern — the Tauri IPC/dialog mocks in src/test-setup.ts
// are reused, not reinvented (this file deliberately adds no vi.mock of its
// own so the setup mocks stay intact).
//
// jsdom axe disables, each with a one-line justification (same honesty
// standard as the spec snippet, nothing broader):
// - color-contrast: jsdom has no computed colors; enforced by
//   src/__tests__/a11y-contrast.test.ts instead.
// - aria-allowed-role: axe 4.13 flags `<aside role="dialog">` (PeekPanel,
//   ActivityFeedPanel) because the legacy HTML element mapping predates ARIA
//   dialogs; the role itself carries full dialog semantics and is the
//   accessible pattern these components intentionally use.
// A green run here is NOT a full conformance claim — see
// docs/a11y/manual-checklist.md.
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { createElement } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import axe from "axe-core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";

import { FactPowerMenu } from "../components/brain/FactPowerMenu";
import { PeekPanel } from "../components/shell/PeekPanel";
import { CommandPalette } from "../components/shell/CommandPalette";
import { OkfInteropBar } from "../components/shell/OkfInteropBar";
import { EphemeralDisclosureModal } from "../components/privacy/EphemeralDisclosureModal";
import { MigrationDisclosureModal } from "../components/privacy/MigrationDisclosureModal";
import { registerCommandContext } from "../lib/commands";
import { __resetWikilinkResolverForTests } from "../components/brain/WikilinkText";
import {
  AnnouncerProvider,
  useAnnouncer,
  SkipLink,
  VisuallyHidden,
} from "../a11y";
import type { EntityFact } from "../lib/tauri";

const invokeMock = vi.mocked(invoke);

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

const OKF_PREVIEW = {
  profile: "llm-wiki/1",
  warnings: [],
  entities: [
    {
      entity_id: "ent_a",
      name: "Project X",
      entity_exists: false,
      facts_new: 3,
      facts_existing: 0,
      tasks_new: 1,
      tasks_existing: 0,
      edges_total: 2,
      events_new: 4,
      events_duplicate: 0,
      summary_action: "fill",
    },
  ],
};

/** Runs axe with only the two documented jsdom disables and asserts clean. */
async function expectNoAxeViolations(container: HTMLElement) {
  const results = await axe.run(container, {
    rules: {
      "color-contrast": { enabled: false }, // jsdom has no computed colors
      "aria-allowed-role": { enabled: false }, // <aside role="dialog"> (see header)
    },
  });
  const summary = results.violations.map(
    (v) => `${v.id}(${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`,
  );
  expect(summary, `axe violations: ${summary.join(" | ")}`).toEqual([]);
}

describe("a11y: axe-core (jsdom, real composed surfaces)", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(null);
    vi.mocked(save).mockReset();
    vi.mocked(open).mockReset();
    __resetWikilinkResolverForTests();
  });

  it("PeekPanel (trapped dialog) has zero axe violations", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "fetch_chunk_content") return Promise.resolve("the exact passage");
      return Promise.resolve(null);
    });
    const { container } = render(
      createElement(PeekPanel, {
        target: { path: "documents/notes.md", hash: "abc123" },
        onDismiss: () => {},
        onPromote: () => {},
      }),
    );
    // Assert the surface really rendered before axe runs: dialog present and
    // the async chunk content landed.
    expect(screen.getByRole("dialog", { name: "Source peek: notes.md" })).toBeInTheDocument();
    expect(await screen.findByText("the exact passage")).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it("CommandPalette has zero axe violations", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "list_entities_cmd") return Promise.resolve([]);
      if (cmd === "list_vault_files") return Promise.resolve([]);
      return Promise.resolve(null);
    });
    const navigate = vi.fn();
    const unregister = registerCommandContext({ navigate });
    try {
      const { container } = render(
        createElement(CommandPalette, { scope: "mode:brain", onClose: () => {} }),
      );
      expect(screen.getByRole("combobox", { name: "Search commands" })).toBeInTheDocument();
      // The listbox must be non-empty (registry commands) — an empty render
      // would let axe pass vacuously.
      expect(screen.getAllByRole("option").length).toBeGreaterThan(0);
      await expectNoAxeViolations(container);
    } finally {
      unregister();
    }
  });

  it("OkfInteropBar with import-preview dialog has zero axe violations", async () => {
    vi.mocked(open).mockResolvedValue("/tmp/brain-okf.zip");
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "okf_import_preview_cmd") return Promise.resolve(OKF_PREVIEW);
      return Promise.resolve(null);
    });
    const { container } = render(createElement(OkfInteropBar));
    expect(screen.getByRole("button", { name: /export brain/i })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /import bundle/i }));
    expect(await screen.findByRole("dialog", { name: "Import preview" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /Merge \(add new, keep existing\)/ })).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it("ActivityFeedPanel (modal aside) has zero axe violations", async () => {
    const { ActivityFeedPanel } = await import("../components/shell/ActivityFeedPanel");
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "list_events_cmd") return Promise.resolve([]);
      return Promise.resolve(null);
    });
    const { container } = render(
      createElement(ActivityFeedPanel, {
        isOpen: true,
        onClose: () => {},
        onNavigate: () => {},
        errors: [],
      }),
    );
    expect(screen.getByRole("dialog", { name: "Activity feed" })).toBeInTheDocument();
    expect(screen.getAllByRole("button").length).toBeGreaterThan(0);
    await expectNoAxeViolations(container);
  });

  it("FactPowerMenu has zero axe violations", async () => {
    const { container } = render(
      createElement(FactPowerMenu, { fact: baseFact, open: true, onClose: () => {} }),
    );
    // Provenance content really mounted — not a violation-free empty render.
    expect(screen.getByText("fact_x")).toBeInTheDocument();
    expect(screen.getByText("human:alice")).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it("EphemeralDisclosureModal has zero axe violations", async () => {
    const { container } = render(
      createElement(EphemeralDisclosureModal, {
        onAcknowledged: () => {},
        onCancel: () => {},
      }),
    );
    expect(screen.getByRole("dialog", { name: "What leaves your machine" })).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it("MigrationDisclosureModal has zero axe violations", async () => {
    const { container } = render(
      createElement(MigrationDisclosureModal, { onAcknowledged: () => {} }),
    );
    expect(
      screen.getByRole("dialog", { name: "Connected agent privacy" }),
    ).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it("primitives (AnnouncerProvider + SkipLink + VisuallyHidden) have zero axe violations", async () => {
    function AnnounceButton() {
      const { announce } = useAnnouncer();
      return createElement(
        "button",
        { type: "button", onClick: () => announce("Export finished") },
        "Announce now",
      );
    }
    const { container } = render(
      createElement(
        AnnouncerProvider,
        null,
        createElement(
          "div",
          null,
          createElement(SkipLink, { targetId: "main-content" }),
          createElement(
            "main",
            { id: "main-content" },
            createElement("h1", null, "Library"),
            createElement(VisuallyHidden, null, "Screen-reader-only hint"),
            createElement(AnnounceButton),
          ),
        ),
      ),
    );
    const skip = screen.getByRole("link", { name: "Skip to main content" });
    expect(screen.getByRole("main")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Announce now" }));
    // The announcement must actually land in the live region for axe to see it.
    expect(await screen.findByText("Export finished")).toBeInTheDocument();
    expect(skip).toHaveAttribute("href", "#main-content");
    await expectNoAxeViolations(container);
  });

  afterEach(() => {
    // PeekPanel's focus trap/inert guard listen on window; drop leftovers so
    // suites stay isolated.
    document.body.innerHTML = "";
  });
});
