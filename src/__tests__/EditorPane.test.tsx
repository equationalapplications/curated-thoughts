import { screen, waitFor, fireEvent } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";

type MockBlock = {
  id: string;
  type: string;
  props?: Record<string, unknown>;
  content: Array<{ type: string; text?: string }>;
  children: unknown[];
};

const DEFAULT_BLOCKS: MockBlock[] = [
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

// Heading and paragraph share the same text. The anchor resolver must
// skip the paragraph and target the heading.
const DUPLICATE_TEXT_DOC = "# chunk-42\n\nchunk-42";
const DUPLICATE_TEXT_BLOCKS: MockBlock[] = [
  {
    id: "block-dup-heading",
    type: "heading",
    content: [{ type: "text", text: "chunk-42" }],
    children: [],
  },
  {
    id: "block-dup-paragraph",
    type: "paragraph",
    content: [{ type: "text", text: "chunk-42" }],
    children: [],
  },
];

const A_BLOCKS: MockBlock[] = [
  {
    id: "a-heading",
    type: "heading",
    content: [{ type: "text", text: "A heading" }],
    children: [],
  },
  {
    id: "a-body",
    type: "paragraph",
    content: [{ type: "text", text: "A body" }],
    children: [],
  },
];

const B_BLOCKS: MockBlock[] = [
  {
    id: "b-heading",
    type: "heading",
    content: [{ type: "text", text: "B heading" }],
    children: [],
  },
  {
    id: "b-body",
    type: "paragraph",
    content: [{ type: "text", text: "B body" }],
    children: [],
  },
];

const editorMock = {
  document: [] as MockBlock[],
  tryParseMarkdownToBlocks: vi.fn(async (content: string): Promise<MockBlock[]> => {
    // The stale-resolution test verifies that doc B's blocks (not A's)
    // are applied, so the parser must distinguish documents by content.
    if (content === "# A content") return A_BLOCKS;
    if (content === "# B content") return B_BLOCKS;
    if (content === DUPLICATE_TEXT_DOC) return DUPLICATE_TEXT_BLOCKS;
    return DEFAULT_BLOCKS;
  }),
  replaceBlocks: vi.fn((_doc: unknown, blocks: MockBlock[]) => {
    editorMock.document = blocks;
  }),
  blocksToMarkdownLossy: vi.fn(async () => "# Saved"),
  setTextCursorPosition: vi.fn(),
};

vi.mock("@blocknote/react", () => ({
  useCreateBlockNote: () => editorMock,
}));

vi.mock("@blocknote/mantine", () => ({
  BlockNoteView: ({ editor }: { editor: typeof editorMock }) => (
    <div data-testid="blocknote">
      {editor.document.map((b: MockBlock) => (
        <div key={b.id} data-id={b.id} />
      ))}
    </div>
  ),
}));

import { EditorPane } from "../components/shell/EditorPane";
import { renderWithTheme } from "./test-utils";
import { ThemeProvider } from "../lib/ThemeContext";

// jsdom does not implement scrollIntoView. Define it as a noop on
// HTMLElement.prototype so the anchor effect can call it without
// throwing. Each test spies on the instance method to assert calls.
if (
  typeof HTMLElement !== "undefined" &&
  !HTMLElement.prototype.hasOwnProperty("scrollIntoView")
) {
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    value: function scrollIntoView() {},
    writable: true,
    configurable: true,
  });
}

let scrollIntoViewSpy: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  editorMock.tryParseMarkdownToBlocks.mockClear();
  editorMock.replaceBlocks.mockClear();
  editorMock.blocksToMarkdownLossy.mockClear();
  editorMock.setTextCursorPosition.mockClear();
  // Reset the mock document so each test sees a clean state.
  editorMock.document = [];
  scrollIntoViewSpy = vi
    .spyOn(HTMLElement.prototype, "scrollIntoView")
    .mockImplementation(() => {});
});

afterEach(() => {
  scrollIntoViewSpy.mockRestore();
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
  // setTextCursorPosition only moves the selection; the resolved DOM node
  // must also be scrolled into view so the user can see the anchor.
  await waitFor(() => {
    const calls = scrollIntoViewSpy.mock.calls;
    expect(calls.length).toBeGreaterThan(0);
    const target = calls.find((args) => args[0] === calls[0]?.[0]);
    expect(target).toBeDefined();
  });
  // Verify the heading block (not the paragraph) was the scroll target.
  const headingEl = document.querySelector('[data-id="block-heading"]');
  expect(headingEl).not.toBeNull();
  expect(scrollIntoViewSpy.mock.instances).toContain(headingEl);
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

test("anchors to heading when paragraph shares anchorChunkId text", async () => {
  // Regression for the case where a paragraph block earlier in the document
  // happens to use the same text as the target heading.
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") {
      return Promise.resolve(DUPLICATE_TEXT_DOC);
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

  await waitFor(() =>
    expect(editorMock.tryParseMarkdownToBlocks).toHaveBeenCalled(),
  );
  // The cursor must land on the heading, not on the duplicate-text
  // paragraph above it.
  await waitFor(() =>
    expect(editorMock.setTextCursorPosition).toHaveBeenCalledWith(
      "block-dup-heading",
      "end",
    ),
  );
  // And the scrollIntoView target must be the heading DOM node.
  const headingEl = document.querySelector('[data-id="block-dup-heading"]');
  expect(headingEl).not.toBeNull();
  expect(scrollIntoViewSpy.mock.instances).toContain(headingEl);
});

test("ignores stale doc-load resolution after selectedDoc changes", async () => {
  // Simulate a race: doc A's readDocument resolves AFTER doc B's. The
  // cancellation guard must prevent A from clobbering B's editor blocks
  // and load state.
  let resolveA: (value: string) => void = () => {};
  const aPromise = new Promise<string>((resolve) => {
    resolveA = resolve;
  });
  const calls: string[] = [];
  vi.mocked(invoke).mockImplementation((cmd: string, args?: unknown) => {
    if (cmd === "read_document") {
      const path = (args as { path?: string } | undefined)?.path ?? "";
      calls.push(path);
      if (path === "A.md") return aPromise;
      if (path === "B.md") return Promise.resolve("# B content");
    }
    return Promise.resolve(null);
  });

  const { rerender } = renderWithTheme(
    <EditorPane selectedDoc="A.md" isWiki={true} />,
  );
  // Switch to B before A resolves. The rerender call must pass the full
  // wrapped element (ThemeProvider) because renderWithTheme wraps the input.
  rerender(
    <ThemeProvider>
      <EditorPane selectedDoc="B.md" isWiki={true} />
    </ThemeProvider>,
  );

  // Now resolve A. The cancellation guard should drop this update.
  resolveA("# A content");
  await waitFor(() =>
    expect(editorMock.tryParseMarkdownToBlocks).toHaveBeenCalled(),
  );
  // readDocument must have been called for both A and B in order.
  expect(calls).toEqual(["A.md", "B.md"]);
  // Only B's resolution should have produced a replaceBlocks mutation.
  // If the cancellation guard regresses, A's resolution also calls
  // replaceBlocks and this count becomes 2.
  expect(editorMock.replaceBlocks).toHaveBeenCalledTimes(1);
  // And the single replacement must carry B's blocks, not A's. The mock
  // returns distinct blocks per content, so we can match by the second
  // argument (the parsed blocks).
  const lastCall = editorMock.replaceBlocks.mock.calls[0]!;
  expect(lastCall[1]).toEqual(B_BLOCKS);
  expect(editorMock.document).toEqual(B_BLOCKS);
});