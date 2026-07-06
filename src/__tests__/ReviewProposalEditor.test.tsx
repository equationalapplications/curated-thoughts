import { screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { ReviewProposalEditor } from "../components/review/ReviewProposalEditor";
import type { ReviewPage } from "../lib/tauri";
import { renderWithTheme } from "./test-utils";

const editorMock = {
  document: [],
  tryParseMarkdownToBlocks: vi.fn(async () => []),
  replaceBlocks: vi.fn(),
  blocksToMarkdownLossy: vi.fn(async () => "# Edited Wiki Page\n\nEdited content."),
  onChange: vi.fn(() => () => {}),
};

vi.mock("@blocknote/react", () => ({
  useCreateBlockNote: () => editorMock,
}));

vi.mock("@blocknote/mantine", () => ({
  BlockNoteView: () => <div data-testid="blocknote-editor" />,
}));

const PAGE = {
  id: 1,
  path: "wiki/Project-X.md",
  generated_by: "llama3.2:3b",
  source_doc_ids: "[]",
} as unknown as ReviewPage;

const PROPOSED = "# Test Wiki Page\n\nTest content.";

beforeEach(() => {
  editorMock.tryParseMarkdownToBlocks.mockClear();
  editorMock.replaceBlocks.mockClear();
  editorMock.blocksToMarkdownLossy.mockClear();
  editorMock.onChange.mockClear();
  vi.mocked(invoke).mockReset();
});

test("shows BlockNote editor for new proposals when wiki page does not exist", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") return Promise.reject(new Error("ENOENT"));
    return Promise.resolve(null);
  });

  const onEditedContentChange = vi.fn();
  renderWithTheme(
    <ReviewProposalEditor
      page={PAGE}
      proposedContent={PROPOSED}
      onEditedContentChange={onEditedContentChange}
    />,
  );

  expect(await screen.findByTestId("blocknote-editor")).toBeInTheDocument();
  expect(screen.getByTestId("review-proposal-editor")).toHaveAttribute(
    "data-variant",
    "new",
  );
  await waitFor(() =>
    expect(onEditedContentChange).toHaveBeenCalledWith(PROPOSED),
  );
});

test("shows ProposalDiff for updates when wiki page already exists", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") {
      return Promise.resolve("# Project X\n\nExisting wiki content.");
    }
    return Promise.resolve(null);
  });

  const onEditedContentChange = vi.fn();
  renderWithTheme(
    <ReviewProposalEditor
      page={PAGE}
      proposedContent={PROPOSED}
      onEditedContentChange={onEditedContentChange}
    />,
  );

  const diff = await screen.findByTestId("proposal-diff");
  expect(diff).toBeInTheDocument();
  expect(screen.getByTestId("review-proposal-editor")).toHaveAttribute(
    "data-variant",
    "update",
  );
  expect(screen.getByText(/test wiki page/i)).toBeInTheDocument();
  await waitFor(() =>
    expect(onEditedContentChange).toHaveBeenCalledWith(PROPOSED),
  );
});

test("shows loading state while proposed content is null", () => {
  renderWithTheme(
    <ReviewProposalEditor
      page={PAGE}
      proposedContent={null}
      onEditedContentChange={vi.fn()}
    />,
  );
  expect(screen.getByText(/loading proposal/i)).toBeInTheDocument();
});
