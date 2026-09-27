import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("../hooks/useSearch", () => ({
  useSearch: () => ({ query: "", setQuery: vi.fn(), results: [], searching: false }),
}));
vi.mock("../hooks/useVaultFiles", () => ({ useVaultFiles: () => [] }));

import { LibraryMode } from "../components/modes/LibraryMode";

describe("LibraryMode first-run empty state", () => {
  it("does not promise local-only processing, which depends on the privacy mode", () => {
    render(<LibraryMode vaultPath="/vault" selectedDoc={null} onDocSelect={vi.fn()} onPickFile={vi.fn()} />);
    expect(screen.getByText("Drop your first document")).toBeInTheDocument();
    expect(screen.getByText(/the files stay where they are/)).toBeInTheDocument();
    expect(screen.queryByText(/leaves your machine/)).not.toBeInTheDocument();
  });
});
