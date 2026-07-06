import { screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { StatusBar } from "../components/shell/StatusBar";
import { renderWithTheme } from "./test-utils";

const noop = vi.fn();

test("shows Idle and the vault folder name by default", async () => {
  renderWithTheme(
    <StatusBar
      vaultPath="/Users/test/Curated-Thoughts"
      onOpenActivity={noop}
      onOpenPrivacy={noop}
    />,
  );
  expect(await screen.findByText(/Idle/)).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: /Vault: Curated-Thoughts/i }),
  ).toBeInTheDocument();
});

test("shows embedding progress when files are pending", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) =>
    cmd === "get_indexing_status"
      ? Promise.resolve({ indexed: 2, pending: 3 })
      : Promise.resolve(null),
  );
  renderWithTheme(
    <StatusBar
      vaultPath="/Users/test/Curated-Thoughts"
      onOpenActivity={noop}
      onOpenPrivacy={noop}
    />,
  );
  expect(await screen.findByText("Embedding 3 files…")).toBeInTheDocument();
});

test("shows indexed count when idle with indexed docs", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) =>
    cmd === "get_indexing_status"
      ? Promise.resolve({ indexed: 5, pending: 0 })
      : Promise.resolve(null),
  );
  renderWithTheme(
    <StatusBar
      vaultPath="/Users/test/Curated-Thoughts"
      onOpenActivity={noop}
      onOpenPrivacy={noop}
    />,
  );
  await waitFor(() =>
    expect(screen.getByText("Idle — 5 docs indexed")).toBeInTheDocument(),
  );
});

test("clicking librarian state opens activity feed", async () => {
  const onOpenActivity = vi.fn();
  renderWithTheme(
    <StatusBar
      vaultPath="/Users/test/Curated-Thoughts"
      onOpenActivity={onOpenActivity}
      onOpenPrivacy={noop}
    />,
  );
  await screen.findByText(/Idle/);
  screen.getByRole("button", { name: /^Idle/i }).click();
  expect(onOpenActivity).toHaveBeenCalled();
});

test("clicking privacy shield opens privacy settings", async () => {
  const onOpenPrivacy = vi.fn();
  renderWithTheme(
    <StatusBar
      vaultPath="/Users/test/Curated-Thoughts"
      onOpenActivity={noop}
      onOpenPrivacy={onOpenPrivacy}
    />,
  );
  await screen.findByText(/Idle/);
  screen.getByLabelText(/Strict privacy/i).click();
  expect(onOpenPrivacy).toHaveBeenCalled();
});
