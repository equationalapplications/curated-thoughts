import { describe, it, expect } from "vitest";
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

  it("copy button is enabled and clickable", async () => {
    const user = userEvent.setup();
    render(<AgentIntegrationPanel brainDir="/Users/test/.brain" />);
    const button = screen.getByRole("button", { name: /copy/i }) as HTMLButtonElement;
    expect(button.disabled).toBe(false);
    await user.click(button);
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
