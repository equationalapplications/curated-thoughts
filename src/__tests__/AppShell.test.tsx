import { screen, fireEvent } from "@testing-library/react";

vi.mock("../components/shell/EditorPane", () => ({
  EditorPane: () => <div data-testid="editor-pane" />,
}));

import { AppShell } from "../components/shell/AppShell";
import { renderWithTheme } from "./test-utils";

const VAULT = "/Users/test/Curated-Thoughts";

function renderAppShell(overrides: Partial<React.ComponentProps<typeof AppShell>> = {}) {
  return renderWithTheme(
    <AppShell
      vaultPath={VAULT}
      onVaultChanged={vi.fn()}
      needsSetup={false}
      {...overrides}
    />,
  );
}

test("opens in Brain mode with rail and status bar", async () => {
  renderAppShell();
  expect(screen.getByRole("button", { name: "Brain" })).toHaveAttribute(
    "aria-current",
    "page",
  );
  expect(
    await screen.findByRole("button", { name: /Vault: Curated-Thoughts/i }),
  ).toBeInTheDocument();
});

test("clicking Review in the rail shows the review screen", async () => {
  renderAppShell();
  fireEvent.click(screen.getByRole("button", { name: "Review" }));
  expect(await screen.findByText(/queue clear/i)).toBeInTheDocument();
});

test("clicking Settings shows the settings screen", () => {
  renderAppShell();
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  expect(screen.getByRole("tab", { name: "Vault" })).toBeInTheDocument();
});

test("Cmd+3 switches to Library mode", () => {
  renderAppShell();
  fireEvent.keyDown(window, { key: "3", metaKey: true });
  expect(screen.getByRole("button", { name: "Library" })).toHaveAttribute(
    "aria-current",
    "page",
  );
});

test("status bar privacy shield navigates to Privacy settings", async () => {
  renderAppShell();
  await screen.findByText(/Idle/);
  fireEvent.click(screen.getByLabelText(/Strict privacy/i));
  expect(screen.getByRole("tab", { name: "Privacy" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

test("status bar librarian segment opens activity panel", async () => {
  renderAppShell();
  await screen.findByText(/Idle/);
  fireEvent.click(screen.getByRole("button", { name: /^Idle/i }));
  expect(screen.getByRole("dialog", { name: "Activity feed" })).toBeInTheDocument();
});

test("back button returns to previous mode after navigation", async () => {
  renderAppShell();
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

// Lands in Task 9 — VaultPanel doesn't yet have the Re-run button
it.skip("navigating to setup mode mounts the wizard", async () => {
  renderAppShell();
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  fireEvent.click(screen.getByRole("tab", { name: "Vault" }));
  const rerunButton = await screen.findByRole("button", { name: /re-run setup wizard/i });
  fireEvent.click(rerunButton);
  expect(await screen.findByRole("region", { name: /where is your vault/i })).toBeInTheDocument();
});