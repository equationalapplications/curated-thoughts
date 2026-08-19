import { screen, waitFor, fireEvent } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";

const editorMock = {
  document: [],
  tryParseMarkdownToBlocks: vi.fn(async () => {
    // Return two blocks so we can pick the one matching the anchor.
    return [
      {
        id: "block-heading",
        type: "heading",
        props: { level: 2 },
        content: [{ type: "text", text: "chunk-42" }],
        children: [],
      },
      {
        id: "block-body",
        type: "paragraph",
        content: [{ type: "text", text: "Body text" }],
        children: [],
      },
    ];
  }),
  replaceBlocks: vi.fn(),
  blocksToMarkdownLossy: vi.fn(async () => "# Saved"),
  setTextCursorPosition: vi.fn(),
};

vi.mock("@blocknote/react", () => ({
  useCreateBlockNote: () => editorMock,
}));

vi.mock("@blocknote/mantine", () => ({
  BlockNoteView: ({ editor }: { editor: typeof editorMock }) => (
    <div data-testid="blocknote">
      <div data-id={editor.document[0]?.id} />
    </div>
  ),
}));

import { EditorPane } from "../components/shell/EditorPane";
import { renderWithTheme } from "./test-utils";

beforeEach(() => {
  editorMock.tryParseMarkdownToBlocks.mockClear();
  editorMock.replaceBlocks.mockClear();
  editorMock.blocksToMarkdownLossy.mockClear();
  editorMock.setTextCursorPosition.mockClear();
  // Reset the mock document so each test sees a clean state.
  editorMock.document = [];
});

test("shows load error when read_document fails", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") return Promise.reject(new Error("ENOENT"));
    return Promise.resolve(null);
  });

  renderWithTheme(
    <EditorPane selectedDoc="wiki/Missing.md" isWiki={true} />,
  );

  expect(await screen.findByRole("alert")).toHaveTextContent("ENOENT");
  expect(screen.queryByTestId("blocknote")).not.toBeInTheDocument();
});

test("shows save error when save_wiki_page fails", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") return Promise.resolve("# Hello");
    if (cmd === "save_wiki_page") return Promise.reject(new Error("disk full"));
    return Promise.resolve(null);
  });

  renderWithTheme(
    <EditorPane selectedDoc="wiki/Page.md" isWiki={true} />,
  );

  await waitFor(() => expect(screen.getByTestId("blocknote")).toBeInTheDocument());
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() =>
    expect(screen.getByRole("alert")).toHaveTextContent("disk full"),
  );
  expect(vi.mocked(invoke)).toHaveBeenCalledWith(
    "save_wiki_page",
    expect.objectContaining({ path: "Page.md", content: "# Saved" }),
  );
});

test("scrolls to anchor when anchorChunkId is provided", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") {
      return Promise.resolve("# chunk-42\n\nBody text");
    }
    return Promise.resolve(null);
  });

  renderWithTheme(
    <EditorPane
      selectedDoc="documents/notes.md"
      isWiki={false}
      anchorChunkId="chunk-42"
    />,
  );

  // Wait for the document to load and parse.
  await waitFor(() =>
    expect(editorMock.tryParseMarkdownToBlocks).toHaveBeenCalled(),
  );
  await waitFor(() =>
    expect(editorMock.setTextCursorPosition).toHaveBeenCalledWith(
      "block-heading",
      "end",
    ),
  );
});

test("does not scroll when anchorChunkId is omitted", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") return Promise.resolve("# chunk-42\n\nBody");
    return Promise.resolve(null);
  });

  renderWithTheme(
    <EditorPane selectedDoc="documents/notes.md" isWiki={false} />,
  );

  await waitFor(() =>
    expect(editorMock.tryParseMarkdownToBlocks).toHaveBeenCalled(),
  );
  // Give the rAF tick a chance to run.
  await new Promise((r) => setTimeout(r, 10));
  expect(editorMock.setTextCursorPosition).not.toHaveBeenCalled();
});

test("does not throw when anchorChunkId has no matching block", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") return Promise.resolve("# other-heading\n\nBody");
    return Promise.resolve(null);
  });

  renderWithTheme(
    <EditorPane
      selectedDoc="documents/notes.md"
      isWiki={false}
      anchorChunkId="missing-chunk"
    />,
  );

  await waitFor(() =>
    expect(editorMock.tryParseMarkdownToBlocks).toHaveBeenCalled(),
  );
  await new Promise((r) => setTimeout(r, 10));
  expect(editorMock.setTextCursorPosition).not.toHaveBeenCalled();
});