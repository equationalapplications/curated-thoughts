import { screen, fireEvent } from "@testing-library/react";

vi.mock("../components/shell/EditorPane", () => ({
  EditorPane: () => <div data-testid="editor-pane" />,
}));

import { AppShell } from "../components/shell/AppShell";
import { renderWithTheme } from "./test-utils";

const VAULT = "/Users/test/Curated-Thoughts";

test("opens in Brain mode with rail and status bar", async () => {
  renderWithTheme(<AppShell vaultPath={VAULT} onVaultChanged={vi.fn()} />);
  expect(screen.getByRole("button", { name: "Brain" })).toHaveAttribute(
    "aria-current",
    "page",
  );
  expect(
    await screen.findByRole("button", { name: /Vault: Curated-Thoughts/i }),
  ).toBeInTheDocument();
});

test("clicking Review in the rail shows the review screen", async () => {
  renderWithTheme(<AppShell vaultPath={VAULT} onVaultChanged={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Review" }));
  expect(await screen.findByText(/queue clear/i)).toBeInTheDocument();
});

test("clicking Settings shows the settings screen", () => {
  renderWithTheme(<AppShell vaultPath={VAULT} onVaultChanged={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  expect(screen.getByRole("tab", { name: "Vault" })).toBeInTheDocument();
});

test("Cmd+3 switches to Library mode", () => {
  renderWithTheme(<AppShell vaultPath={VAULT} onVaultChanged={vi.fn()} />);
  fireEvent.keyDown(window, { key: "3", metaKey: true });
  expect(screen.getByRole("button", { name: "Library" })).toHaveAttribute(
    "aria-current",
    "page",
  );
});

test("status bar privacy shield navigates to Privacy settings", async () => {
  renderWithTheme(<AppShell vaultPath={VAULT} onVaultChanged={vi.fn()} />);
  await screen.findByText(/Idle/);
  fireEvent.click(screen.getByLabelText(/Strict privacy/i));
  expect(screen.getByRole("tab", { name: "Privacy" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

test("status bar librarian segment opens activity panel", async () => {
  renderWithTheme(<AppShell vaultPath={VAULT} onVaultChanged={vi.fn()} />);
  await screen.findByText(/Idle/);
  fireEvent.click(screen.getByRole("button", { name: /^Idle/i }));
  expect(screen.getByRole("dialog", { name: "Activity feed" })).toBeInTheDocument();
});

test("back button returns to previous mode after navigation", async () => {
  renderWithTheme(<AppShell vaultPath={VAULT} onVaultChanged={vi.fn()} />);
  expect(screen.getByRole("button", { name: "Brain" })).toHaveAttribute(
    "aria-current",
    "page"
  );

  // Navigate to Review
  fireEvent.click(screen.getByRole("button", { name: "Review" }));
  await screen.findByText(/queue clear/i);
  expect(screen.getByRole("button", { name: "Review" })).toHaveAttribute(
    "aria-current",
    "page"
  );

  // Click back button
  const backButton = screen.getByRole("button", { name: "Go back" });
  expect(backButton).not.toBeDisabled();
  fireEvent.click(backButton);

  // Should return to Brain
  expect(screen.getByRole("button", { name: "Brain" })).toHaveAttribute(
    "aria-current",
    "page"
  );
});
