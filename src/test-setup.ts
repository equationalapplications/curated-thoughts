import "@testing-library/jest-dom";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string) => {
    if (cmd === "check_ollama") {
      return Promise.resolve({ installed: true, running: true, models: ["llama3.2:3b"] });
    }
    if (cmd === "get_recommended_model") {
      return Promise.resolve("llama3.2:3b");
    }
    if (cmd === "get_indexing_status") {
      return Promise.resolve({ indexed: 0, pending: 0 });
    }
    if (cmd === "wiki_exec") return Promise.resolve(null);
    if (cmd === "wiki_run") return Promise.resolve({ changes: 0, last_insert_row_id: 0 });
    if (cmd === "wiki_get_all") return Promise.resolve([]);
    if (cmd === "wiki_get_first") return Promise.resolve(null);
    if (cmd === "embed_text") return Promise.resolve(Array(384).fill(0));
    if (cmd === "ollama_generate") return Promise.resolve("");
    if (cmd === "search_vault") return Promise.resolve([]);
    if (cmd === "get_related_chunks") return Promise.resolve([]);
    if (cmd === "list_vault_files") return Promise.resolve([]);
    if (cmd === "read_document") return Promise.resolve("# Hello\n\nTest document.");
    if (cmd === "get_review_queue") return Promise.resolve([]);
    if (cmd === "approve_wiki_page") return Promise.resolve();
    if (cmd === "reject_wiki_page") return Promise.resolve();
    return Promise.resolve(null);
  }),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
  emit: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-shell", () => ({
  open: vi.fn(),
}));
