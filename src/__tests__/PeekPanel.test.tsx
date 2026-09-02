import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, vi } from "vitest";
import { PeekPanel } from "../components/shell/PeekPanel";

vi.mock("@tauri-apps/api/core");

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue("the exact passage");
});

function renderPanel() {
  const onDismiss = vi.fn();
  const onPromote = vi.fn();
  const view = render(
    <PeekPanel
      target={{ path: "documents/notes.md", hash: "abc123" }}
      onDismiss={onDismiss}
      onPromote={onPromote}
    />,
  );
  return { onDismiss, onPromote, ...view };
}

test("renders fetched chunk text", async () => {
  invokeMock.mockImplementation((_cmd: string, args?: Record<string, unknown>) => {
    expect(args).toEqual({ path: "documents/notes.md", hash: "abc123" });
    return Promise.resolve("the exact passage");
  });
  renderPanel();
  expect(screen.getByText("Loading…")).toBeInTheDocument();
  expect(await screen.findByText("the exact passage")).toBeInTheDocument();
  expect(screen.getByRole("dialog", { name: "Source peek: notes.md" })).toBeInTheDocument();
});

test("Escape dismisses the panel", () => {
  const { onDismiss } = renderPanel();
  // The focus trap listens on the dialog itself; with the background
  // inert, Escape can only originate inside the panel.
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  expect(onDismiss).toHaveBeenCalledTimes(1);
});

test("backdrop click dismisses the panel", () => {
  const { onDismiss } = renderPanel();
  fireEvent.click(screen.getByRole("button", { name: "Close source peek" }));
  expect(onDismiss).toHaveBeenCalledTimes(1);
});

test("Open ↗ calls onPromote with path and hash", () => {
  const { onPromote } = renderPanel();
  fireEvent.click(screen.getByRole("button", { name: "Open ↗" }));
  expect(onPromote).toHaveBeenCalledWith("documents/notes.md", "abc123");
});

test("focus moves to Open ↗ on mount and returns to opener on unmount", () => {
  const opener = render(<button type="button">Opener chip</button>);
  const openerBtn = screen.getByRole("button", { name: "Opener chip" });
  openerBtn.focus();
  expect(document.activeElement).toBe(openerBtn);

  const panel = render(
    <PeekPanel
      target={{ path: "documents/notes.md", hash: "abc123" }}
      onDismiss={vi.fn()}
      onPromote={vi.fn()}
    />,
  );
  const openBtn = screen.getByRole("button", { name: "Open ↗" });
  expect(document.activeElement).toBe(openBtn);

  panel.unmount();
  expect(document.activeElement).toBe(openerBtn);
  opener.unmount();
});

test("Tab cycles within the panel", () => {
  renderPanel();
  const openBtn = screen.getByRole("button", { name: "Open ↗" });
  openBtn.focus();
  // The Open button is the panel's only focusable element, so Tab wraps
  // straight back onto it instead of escaping to the page behind.
  fireEvent.keyDown(window, { key: "Tab" });
  expect(document.activeElement).toBe(openBtn);
});

test("not-found renders the source-moved notice and keeps the panel open", async () => {
  invokeMock.mockResolvedValue(null);
  const { onDismiss } = renderPanel();
  expect(await screen.findByText(/source may have moved/i)).toBeInTheDocument();
  expect(screen.getByRole("dialog")).toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  expect(onDismiss).toHaveBeenCalledTimes(1);
});

test("backend failure renders the error alert", async () => {
  invokeMock.mockRejectedValue(new Error("db locked"));
  renderPanel();
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Could not load this passage.",
  );
});

// jsdom does not implement `inert`, so it cannot observe the guard directly.
// This pins the DOM witness the guard leaves behind (aria-hidden on the
// panel's PARENT, distinguishing guard scope from the dialog's own subtree):
// on unmount, the guard's cleanup must have run BEFORE the trap's
// restore-focus cleanup — i.e. when focus returns to the opener, the witness
// must already be gone. Reversed order means the restore targeted an inert
// (unfocusable) opener and silently no-ops in every real WebView (WCAG 2.4.3).
test("unmount releases the guard before restoring focus (release-before-restore)", () => {
  const opener = render(<button type="button">Order opener</button>);
  const openerBtn = screen.getByRole("button", { name: "Order opener" });
  openerBtn.focus();

  const panel = render(
    <PeekPanel
      target={{ path: "documents/notes.md", hash: "abc123" }}
      onDismiss={vi.fn()}
      onPromote={vi.fn()}
    />,
  );
  const panelEl = screen.getByRole("dialog");
  // The guard aria-hides side branches off the dialog→body path. The opener
  // lives in its own top-level container (a sibling of the panel's RTL
  // container), so THAT node is the observable witness of guard state.
  const panelContainer = panelEl.parentElement;
  const witness = Array.from(document.body.children).find(
    (el) => el !== panelContainer && el.contains(openerBtn),
  )!;
  expect(witness.getAttribute("aria-hidden")).toBe("true");

  // Record the unmount cleanup order: guard release (aria-hidden removed
  // from the witness) vs trap restore (focus() called on the opener).
  const order: string[] = [];
  const focusSpy = vi
    .spyOn(openerBtn, "focus")
    .mockImplementation(() => void order.push("restore"));
  const origRemove = witness.removeAttribute.bind(witness);
  witness.removeAttribute = ((name: string) => {
    if (name === "aria-hidden") order.push("release");
    return origRemove(name);
  }) as typeof witness.removeAttribute;

  panel.unmount();
  // ORDER proof (jsdom-visible, no `inert` needed): record the guard's
  // attribute release and the trap's restore-focus call into one sequence.
  expect(order).toEqual(["release", "restore"]);
  expect(witness.getAttribute("aria-hidden")).toBeNull();
  // (Actual focus RETURN is asserted by the unspy'd restore test above; the
  // spy here intentionally swallows the real focus() to observe the order.)
  focusSpy.mockRestore();
  opener.unmount();
});
