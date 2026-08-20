import "@testing-library/jest-dom";
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { VaultPanel } from "../VaultPanel";

vi.mock("../../../hooks/useVaultSwitcher", () => ({
  useVaultSwitcher: vi.fn(() => ({
    changeVault: vi.fn(),
    switching: false,
    isSystemBusy: false,
  })),
}));

vi.mock("../../../lib/tauri", () => ({
  revealVault: vi.fn(() => Promise.resolve()),
}));

describe("VaultPanel", () => {
  it("renders the re-run setup wizard button when onRerunWizard is provided", () => {
    const onRerunWizard = vi.fn();
    render(<VaultPanel vaultPath="/test/vault" onRerunWizard={onRerunWizard} />);
    expect(screen.getByRole("button", { name: "Re-run setup wizard" })).toBeInTheDocument();
  });

  it("calls onRerunWizard when the re-run setup wizard button is clicked", async () => {
    const user = userEvent.setup();
    const onRerunWizard = vi.fn();
    render(<VaultPanel vaultPath="/test/vault" onRerunWizard={onRerunWizard} />);
    await user.click(screen.getByRole("button", { name: "Re-run setup wizard" }));
    expect(onRerunWizard).toHaveBeenCalledOnce();
  });
});
