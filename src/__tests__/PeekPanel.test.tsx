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
  fireEvent.keyDown(window, { key: "Escape" });
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
  fireEvent.keyDown(window, { key: "Escape" });
  expect(onDismiss).toHaveBeenCalledTimes(1);
});

test("backend failure renders the error alert", async () => {
  invokeMock.mockRejectedValue(new Error("db locked"));
  renderPanel();
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Could not load this passage.",
  );
});
