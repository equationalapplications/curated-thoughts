import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { AgentIntegrationPanel } from "../AgentIntegrationPanel";

describe("AgentIntegrationPanel", () => {
  it("renders a code block containing --mcp", () => {
    render(<AgentIntegrationPanel brainDir="/Users/test/.brain" />);
    const code = screen.getByRole("code");
    expect(code.textContent).toContain("--mcp");
  });

  it("renders the brainDir env var", () => {
    render(<AgentIntegrationPanel brainDir="/Users/test/.brain" />);
    expect(screen.getByRole("code").textContent).toContain("/Users/test/.brain");
  });
});
