import { screen, fireEvent } from "@testing-library/react";
import { SettingsScreen } from "../components/settings/SettingsScreen";
import { renderWithTheme } from "./test-utils";

const TAB_LABELS = [
  "Vault",
  "Privacy",
  "Models",
  "Librarian",
  "Agents",
  "Maintenance",
  "Appearance",
];

test("renders all seven tabs with Vault active by default", () => {
  renderWithTheme(<SettingsScreen vaultPath="/Users/test/Curated-Thoughts" />);
  for (const label of TAB_LABELS) {
    expect(screen.getByRole("tab", { name: label })).toBeInTheDocument();
  }
  expect(screen.getByRole("tab", { name: "Vault" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

test("clicking a tab switches the selected tab", () => {
  renderWithTheme(<SettingsScreen vaultPath="/Users/test/Curated-Thoughts" />);
  fireEvent.click(screen.getByRole("tab", { name: "Models" }));
  expect(screen.getByRole("tab", { name: "Models" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  expect(screen.getByRole("tab", { name: "Vault" })).toHaveAttribute(
    "aria-selected",
    "false",
  );
});

test("initialTab selects the requested tab", () => {
  renderWithTheme(
    <SettingsScreen
      vaultPath="/Users/test/Curated-Thoughts"
      initialTab="appearance"
    />,
  );
  expect(screen.getByRole("tab", { name: "Appearance" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  expect(screen.getByText(/Theme applies to the shell/i)).toBeInTheDocument();
});

test("each tab renders without crashing", () => {
  renderWithTheme(<SettingsScreen vaultPath="/Users/test/Curated-Thoughts" />);
  for (const label of TAB_LABELS) {
    fireEvent.click(screen.getByRole("tab", { name: label }));
  }
});
