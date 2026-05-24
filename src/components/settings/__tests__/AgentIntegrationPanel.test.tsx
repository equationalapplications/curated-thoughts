import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AgentIntegrationPanel } from "../AgentIntegrationPanel";

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

  it("falls back to execCommand when clipboard.writeText rejects", async () => {
    const user = userEvent.setup();
    const originalExecCommand = Object.getOwnPropertyDescriptor(document, "execCommand");
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const documentOverride = document as unknown as { execCommand?: unknown };
    const navigatorOverride = navigator as unknown as { clipboard?: unknown };
    const execCommandMock = vi.fn(() => true);
    const writeTextMock = vi.fn(async () => {
      throw new Error("clipboard rejected");
    });

    Object.defineProperty(document, "execCommand", {
      value: execCommandMock,
      configurable: true,
    });
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: writeTextMock },
      configurable: true,
    });

    try {
      render(<AgentIntegrationPanel brainDir="/Users/test/.brain" />);
      const button = screen.getByRole("button", { name: /copy/i }) as HTMLButtonElement;
      await user.click(button);

      expect(writeTextMock).toHaveBeenCalledWith(expect.any(String));
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

  it("cleans up the fallback textarea when execCommand is unavailable", async () => {
    const user = userEvent.setup();
    const originalExecCommand = Object.getOwnPropertyDescriptor(document, "execCommand");
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const documentOverride = document as unknown as { execCommand?: unknown };
    const navigatorOverride = navigator as unknown as { clipboard?: unknown };
    const execCommandMock = vi.fn(() => {
      throw new Error("copy unsupported");
    });

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
      await user.click(button);

      expect(execCommandMock).toHaveBeenCalledWith("copy");
      expect(await screen.findByText(/Copy failed\./i)).toBeTruthy();
      expect(document.querySelectorAll("textarea").length).toBe(0);
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
