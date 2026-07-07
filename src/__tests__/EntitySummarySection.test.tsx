import { screen, fireEvent, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";

const editorMock = {
  document: [],
  tryParseMarkdownToBlocks: vi.fn(async () => []),
  replaceBlocks: vi.fn(),
  blocksToMarkdownLossy: vi.fn(async () => "Edited summary."),
};

vi.mock("@blocknote/react", () => ({
  useCreateBlockNote: () => editorMock,
}));

vi.mock("@blocknote/mantine", () => ({
  BlockNoteView: () => <div data-testid="blocknote" />,
}));

import { EntitySummarySection } from "../components/brain/EntitySummarySection";
import { renderWithTheme } from "./test-utils";

beforeEach(() => {
  editorMock.tryParseMarkdownToBlocks.mockClear();
  editorMock.replaceBlocks.mockClear();
  editorMock.blocksToMarkdownLossy.mockClear();
});

test("renders summary prose with wikilink chips in view mode", () => {
  const onNavigate = vi.fn();
  renderWithTheme(
    <EntitySummarySection
      entityId="ent_1"
      summary="Owns [[Project X]]."
      onChanged={vi.fn()}
      onNavigateEntity={onNavigate}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Project X" }));
  expect(onNavigate).toHaveBeenCalledWith("Project X");
  expect(screen.queryByTestId("blocknote")).not.toBeInTheDocument();
});

test("edit loads markdown into BlockNote; save round-trips and persists", async () => {
  const onChanged = vi.fn();
  renderWithTheme(
    <EntitySummarySection
      entityId="ent_1"
      summary="Original."
      onChanged={onChanged}
      onNavigateEntity={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Edit summary" }));
  await screen.findByTestId("blocknote");
  expect(editorMock.tryParseMarkdownToBlocks).toHaveBeenCalledWith("Original.");

  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(onChanged).toHaveBeenCalled());
  expect(invoke).toHaveBeenCalledWith("update_entity_summary_cmd", {
    entityId: "ent_1",
    summary: "Edited summary.",
  });
});
