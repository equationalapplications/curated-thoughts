import { screen, waitFor, fireEvent } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";

const HASH_42 = "0123456789abcdef0123456789abcdef"; // 32-char hex
const HASH_MISSING = "ffffffffffffffffffffffffffffffff";

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
  !Object.prototype.hasOwnProperty.call(
    HTMLElement.prototype,
    "scrollIntoView",
  )
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
  // Clear the invoke mock so each test sees only its own IPC calls.
  vi.mocked(invoke).mockReset();
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

test("renders line-range overlay when resolveChunkOverlay returns line range", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") return Promise.resolve("Body text spanning many lines.");
    if (cmd === "resolve_chunk_overlay") {
      return Promise.resolve({ startLine: 1, endLine: 3 });
    }
    return Promise.resolve(null);
  });
  renderWithTheme(
    <EditorPane
      selectedDoc="documents/notes.md"
      isWiki={false}
      anchorChunkId={HASH_42}
    />,
  );
  await waitFor(() => expect(screen.getByTestId("blocknote")).toBeInTheDocument());
  // jsdom reports 0-rect for every element, so the overlay renders
  // with top=0, height=0 — still visible in the DOM.
  expect(await screen.findByTestId("editor-line-overlay")).toBeInTheDocument();
  expect(invoke).toHaveBeenCalledWith(
    "resolve_chunk_overlay",
    { path: "documents/notes.md", hash: HASH_42 },
  );
});

test("renders source-moved-notice when resolveChunkOverlay returns null", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") return Promise.resolve("# Heading\n\nBody");
    if (cmd === "resolve_chunk_overlay") return Promise.resolve(null);
    return Promise.resolve(null);
  });
  renderWithTheme(
    <EditorPane
      selectedDoc="documents/notes.md"
      isWiki={false}
      anchorChunkId={HASH_MISSING}
    />,
  );
  await waitFor(() => expect(screen.getByTestId("blocknote")).toBeInTheDocument());
  expect(await screen.findByText(/source may have moved/i)).toBeInTheDocument();
});

test("hides source-moved-notice when × button is clicked", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") return Promise.resolve("# Heading\n\nBody");
    if (cmd === "resolve_chunk_overlay") return Promise.resolve(null);
    return Promise.resolve(null);
  });
  renderWithTheme(
    <EditorPane
      selectedDoc="documents/notes.md"
      isWiki={false}
      anchorChunkId={HASH_MISSING}
    />,
  );
  await waitFor(() => expect(screen.getByTestId("blocknote")).toBeInTheDocument());
  expect(await screen.findByText(/source may have moved/i)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: /×/u }));
  await waitFor(() =>
    expect(screen.queryByText(/source may have moved/i)).not.toBeInTheDocument(),
  );
});

test("renders nothing when anchorChunkId is null", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") return Promise.resolve("# Heading");
    return Promise.resolve(null);
  });
  renderWithTheme(
    <EditorPane selectedDoc="documents/notes.md" isWiki={false} />,
  );
  await waitFor(() => expect(screen.getByTestId("blocknote")).toBeInTheDocument());
  expect(screen.queryByText(/source may have moved/i)).not.toBeInTheDocument();
  expect(invoke).not.toHaveBeenCalledWith(
    "resolve_chunk_overlay",
    expect.anything(),
  );
});

test("renders source-moved-notice when line range past EOF cannot map", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") return Promise.resolve("Tiny doc.");
    if (cmd === "resolve_chunk_overlay") {
      return Promise.resolve({ startLine: 900, endLine: 910 });
    }
    return Promise.resolve(null);
  });
  renderWithTheme(
    <EditorPane
      selectedDoc="documents/notes.md"
      isWiki={false}
      anchorChunkId={HASH_42}
    />,
  );
  await waitFor(() => expect(screen.getByTestId("blocknote")).toBeInTheDocument());
  expect(await screen.findByText(/source may have moved/i)).toBeInTheDocument();
});

test("auto-dismisses overlay after 1.5s when visible", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") return Promise.resolve("Block body.");
    if (cmd === "resolve_chunk_overlay") {
      return Promise.resolve({ startLine: 1, endLine: 1 });
    }
    return Promise.resolve(null);
  });
  // Real timers (not fake): the rect-computation effect is rAF-gated,
  // and @testing-library's findByTestId polls via setInterval. Both
  // would hang under vi.useFakeTimers() unless we manually advanced
  // the clock between every wait. Real timers keep the test honest.
  renderWithTheme(
    <EditorPane
      selectedDoc="documents/notes.md"
      isWiki={false}
      anchorChunkId={HASH_42}
    />,
  );
  await waitFor(() =>
    expect(screen.getByTestId("blocknote")).toBeInTheDocument(),
  );
  const overlay = await screen.findByTestId("editor-line-overlay");
  expect(overlay).toBeInTheDocument();
  // The EditorPane's auto-dismiss effect (see EditorPane.tsx) fires a
  // setTimeout(1500ms) when overlayStatus becomes "visible" and calls
  // setDismissed(true), which unmounts the overlay div. After 1.6s the
  // overlay must be gone.
  await new Promise((resolve) => setTimeout(resolve, 1600));
  expect(screen.queryByTestId("editor-line-overlay")).not.toBeInTheDocument();
});

test("re-shows source-moved-notice after a visible overlay auto-dismisses", async () => {
  // Regression test: the auto-dismiss timer sets `dismissed = true`
  // after 1.5s when the overlay becomes visible. If a later anchor
  // resolves to `source-moved`, the presentation effect must reset
  // `dismissed` so the notice can render — otherwise the user sees
  // nothing after the visible overlay times out.
  let overlayCalls = 0;
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "read_document") return Promise.resolve("Some body text.");
    if (cmd === "resolve_chunk_overlay") {
      // First call resolves to a valid line range so the overlay
      // mounts and runs the 1.5s dismissal timer; every subsequent
      // call resolves to null (source-moved).
      return Promise.resolve(
        overlayCalls++ === 0 ? { startLine: 1, endLine: 1 } : null,
      );
    }
    return Promise.resolve(null);
  });
  const { rerender } = renderWithTheme(
    <EditorPane
      selectedDoc="documents/notes.md"
      isWiki={false}
      anchorChunkId={HASH_42}
    />,
  );
  await waitFor(() => expect(screen.getByTestId("blocknote")).toBeInTheDocument());
  // First anchor: resolves to a valid line range, the overlay mounts.
  expect(await screen.findByTestId("editor-line-overlay")).toBeInTheDocument();
  // The auto-dismiss timer fires after 1.5s; advance real timers past it.
  await new Promise((resolve) => setTimeout(resolve, 1600));
  expect(screen.queryByTestId("editor-line-overlay")).not.toBeInTheDocument();
  // Now switch to a different anchor that resolves to source-moved.
  rerender(
    <ThemeProvider>
      <EditorPane
        selectedDoc="documents/notes.md"
        isWiki={false}
        anchorChunkId={HASH_MISSING}
      />
    </ThemeProvider>,
  );
  // The notice must re-appear because the new status is a fresh
  // presentation decision that resets `dismissed` to false.
  expect(await screen.findByText(/source may have moved/i)).toBeInTheDocument();
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