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

  it("copy button writes snippet to clipboard", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    render(<AgentIntegrationPanel brainDir="/Users/test/.brain" />);
    await userEvent.click(screen.getByRole("button", { name: /copy/i }));
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining("--mcp"));
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
