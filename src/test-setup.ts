import "@testing-library/jest-dom";

Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: vi.fn().mockImplementation((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })),
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string) => {
    if (cmd === "check_ollama") {
      return Promise.resolve({ installed: true, running: true, models: ["llama3.2:3b"] });
    }
    if (cmd === "get_vault_path") {
      return Promise.resolve("/Users/test/Curated-Thoughts");
    }
    if (cmd === "get_brain_dir") {
      return Promise.resolve("/Users/test/.brain");
    }
    if (cmd === "get_provider_config") {
      return Promise.resolve({
        generation: {
          provider: "sidecar",
          model_path: null,
          model_name: null,
          external_url: null,
          api_key: null,
        },
        embedding: { provider: "fastembed", external_url: null },
      });
    }
    if (cmd === "update_provider") {
      return Promise.resolve();
    }
    if (cmd === "download_sidecar_engine") {
      return Promise.resolve();
    }
    if (cmd === "download_model_weights") {
      return Promise.resolve();
    }
    if (cmd === "init_fastembed") {
      return Promise.resolve();
    }
    if (cmd === "get_binary_path") {
      return Promise.resolve("/Users/test/Curated Thoughts/curated-thoughts");
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
    if (cmd === "list_proposals_cmd") return Promise.resolve([]);
    if (cmd === "get_folder_rules") return Promise.resolve([]);
    if (cmd === "set_folder_rule") return Promise.resolve();
    if (cmd === "delete_folder_rule") return Promise.resolve();
    if (cmd === "save_wiki_page") return Promise.resolve();
    if (cmd === "delete_vault_file") return Promise.resolve();
    if (cmd === "switch_vault") return Promise.resolve();
    if (cmd === "backup_vault_db") return Promise.resolve("/test/backup.db");
    if (cmd === "check_vault_backup") return Promise.resolve(false);
    if (cmd === "reveal_vault") return Promise.resolve();
    return Promise.resolve(null);
  }),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
  emit: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
  message: vi.fn().mockResolvedValue("No"),
}));

vi.mock("@tauri-apps/plugin-shell", () => ({
  open: vi.fn(),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({
    onDragDropEvent: vi.fn(() => Promise.resolve(() => {})),
    setTitle: vi.fn(() => Promise.resolve()),
  })),
}));
