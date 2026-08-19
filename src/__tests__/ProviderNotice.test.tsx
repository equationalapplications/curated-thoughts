import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ProviderNotice } from "../components/health/ProviderNotice";

describe("ProviderNotice", () => {
  it("renders embedder-down message when embedding is error", () => {
    render(<ProviderNotice feature="search" embedding="error" generation="ok" />);
    expect(screen.getByText(/search needs the embedder/i)).toBeInTheDocument();
  });
  it("renders nothing when both providers are ok", () => {
    const { container } = render(<ProviderNotice feature="search" embedding="ok" generation="ok" />);
    expect(container.firstChild).toBeNull();
  });
  it("renders generation-down message when generation is unconfigured", () => {
    render(<ProviderNotice feature="synthesis" embedding="ok" generation="unconfigured" />);
    expect(screen.getByText(/synthesis needs a generation backend/i)).toBeInTheDocument();
  });
});