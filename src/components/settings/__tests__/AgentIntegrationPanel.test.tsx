import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AgentIntegrationPanel } from "../AgentIntegrationPanel";
import { SettingsModal } from "../SettingsModal";

describe("AgentIntegrationPanel", () => {
  it("renders a code block containing --mcp", () => {
    render(<AgentIntegrationPanel brainDir="/Users/test/.brain" />);
    const code = screen.getByTestId("agent-snippet");
    expect(code.textContent).toContain("--mcp");
  });

  it("renders the brainDir env var", () => {
    render(<AgentIntegrationPanel brainDir="/Users/test/.brain" />);
    expect(screen.getByTestId("agent-snippet").textContent).toContain("/Users/test/.brain");
  });

  it("copy button is enabled and copies the snippet", async () => {
    const user = userEvent.setup();
    const originalExecCommand = Object.getOwnPropertyDescriptor(document, "execCommand");
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const documentOverride = document as unknown as { execCommand?: unknown };
    const navigatorOverride = navigator as unknown as { clipboard?: unknown };
    const execCommandMock = vi.fn(() => true);

    Object.defineProperty(document, "execCommand", {
      value: execCommandMock,
      configurable: true,
    });
    Object.defineProperty(navigator, "clipboard", {
      value: undefined,
      configurable: true,
    });

    try {
      render(<AgentIntegrationPanel brainDir="/Users/test/.brain" />);
      const button = screen.getByRole("button", { name: /copy/i }) as HTMLButtonElement;
      expect(button.disabled).toBe(false);
      await user.click(button);

      expect(execCommandMock).toHaveBeenCalledWith("copy");
      expect(await screen.findByText(/Copied to clipboard\./i)).toBeTruthy();
    } finally {
      if (originalExecCommand) {
        Object.defineProperty(document, "execCommand", originalExecCommand);
      } else {
        delete documentOverride.execCommand;
      }
      if (originalClipboard) {
        Object.defineProperty(navigator, "clipboard", originalClipboard);
      } else {
        delete navigatorOverride.clipboard;
      }
    }
  });
});

it("SettingsModal renders agent integration section heading", () => {
  render(
    <SettingsModal
      onClose={() => {}}
      vaultPath="/test/vault"
    />,
  );
  expect(
    screen.getByText("Developer / Agent Integration"),
  ).toBeTruthy();
});
