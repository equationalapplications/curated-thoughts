import { screen, waitFor, fireEvent } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";

const editorMock = {
  document: [],
  tryParseMarkdownToBlocks: vi.fn(async () => []),
  replaceBlocks: vi.fn(),
  blocksToMarkdownLossy: vi.fn(async () => "# Saved"),
};

vi.mock("@blocknote/react", () => ({
  useCreateBlockNote: () => editorMock,
}));

vi.mock("@blocknote/mantine", () => ({
  BlockNoteView: () => <div data-testid="blocknote" />,
}));

import { EditorPane } from "../components/shell/EditorPane";
import { renderWithTheme } from "./test-utils";

beforeEach(() => {
  editorMock.tryParseMarkdownToBlocks.mockClear();
  editorMock.replaceBlocks.mockClear();
  editorMock.blocksToMarkdownLossy.mockClear();
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
