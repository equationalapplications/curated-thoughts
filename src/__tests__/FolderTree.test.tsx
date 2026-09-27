import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

vi.mock("../lib/tauri", () => ({
  deleteVaultFile: vi.fn().mockResolvedValue(undefined),
  getVaultLayout: vi.fn().mockResolvedValue({
    immutableDir: "immutable-source-files",
    wikiDir: "wiki",
    labels: { immutableDir: "Source Files", wikiDir: "Wiki Pages" },
  }),
}));

import { FolderTree } from "../components/shell/FolderTree";
import { deleteVaultFile } from "../lib/tauri";

const CROSS = "M6 6l12 12M18 6 6 18";

describe("FolderTree delete", () => {
  it("arms with a confirmation mark, not a cross, before deleting", () => {
    render(
      <FolderTree
        files={[{ path: "immutable-source-files/Note.md", name: "Note.md", tier: "user_doc" }]}
        selectedPath={null}
        onSelect={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Delete file" }));
    const confirm = screen.getByRole("button", { name: "Confirm delete" });
    const paths = Array.from(confirm.querySelectorAll("path")).map((p) => p.getAttribute("d"));
    expect(paths).not.toContain(CROSS);
    expect(deleteVaultFile).not.toHaveBeenCalled();

    fireEvent.click(confirm);
    expect(deleteVaultFile).toHaveBeenCalledWith("immutable-source-files/Note.md");
  });
});
