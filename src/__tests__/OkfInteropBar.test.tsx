import { screen, fireEvent, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { OkfInteropBar } from "../components/shell/OkfInteropBar";
import { renderWithTheme } from "./test-utils";

const PREVIEW = {
  profile: "llm-wiki/1",
  warnings: [],
  entities: [
    {
      entity_id: "ent_a",
      name: "Project X",
      entity_exists: false,
      facts_new: 3,
      facts_existing: 0,
      tasks_new: 1,
      tasks_existing: 0,
      edges_total: 2,
      events_new: 4,
      events_duplicate: 0,
      summary_action: "fill",
    },
  ],
};

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(open).mockReset();
  vi.mocked(save).mockReset();
});

test("export flow: save dialog then command, then notice", async () => {
  vi.mocked(save).mockResolvedValue("/tmp/brain-okf.zip");
  vi.mocked(invoke).mockResolvedValue({ path: "/tmp/brain-okf.zip", entities: 2, files: 9 });

  renderWithTheme(<OkfInteropBar />);
  fireEvent.click(screen.getByRole("button", { name: /export brain/i }));

  await waitFor(() =>
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("okf_export_bundle_cmd", {
      destPath: "/tmp/brain-okf.zip",
      entityIds: null,
    }),
  );
  expect(await screen.findByText(/exported 2 entities/i)).toBeInTheDocument();
});

test("export cancelled dialog does nothing", async () => {
  vi.mocked(save).mockResolvedValue(null);
  renderWithTheme(<OkfInteropBar />);
  fireEvent.click(screen.getByRole("button", { name: /export brain/i }));
  await new Promise((r) => setTimeout(r, 20));
  expect(vi.mocked(invoke)).not.toHaveBeenCalled();
});

test("import flow: preview counts shown, apply on confirm", async () => {
  vi.mocked(open).mockResolvedValue("/tmp/incoming.zip");
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "okf_import_preview_cmd") return Promise.resolve(PREVIEW);
    if (cmd === "okf_import_apply_cmd") {
      return Promise.resolve({
        entities_touched: 1,
        facts_added: 3,
        facts_skipped: 0,
        tasks_added: 1,
        tasks_skipped: 0,
        edges_added: 2,
        events_added: 4,
        events_skipped: 0,
      });
    }
    return Promise.resolve(null);
  });
  const onImported = vi.fn();

  renderWithTheme(<OkfInteropBar onImported={onImported} />);
  fireEvent.click(screen.getByRole("button", { name: /import bundle/i }));

  expect(await screen.findByText(/project x/i)).toBeInTheDocument();
  expect(screen.getByText(/3 new facts/i)).toBeInTheDocument();

  fireEvent.click(screen.getByRole("radio", { name: /merge/i }));
  fireEvent.click(screen.getByRole("button", { name: /confirm import/i }));

  await waitFor(() =>
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("okf_import_apply_cmd", {
      srcPath: "/tmp/incoming.zip",
      mode: "merge",
    }),
  );
  expect(await screen.findByText(/imported 3 fact/i)).toBeInTheDocument();
  expect(onImported).toHaveBeenCalled();
});

test("import preview cancel discards without applying", async () => {
  vi.mocked(open).mockResolvedValue("/tmp/incoming.zip");
  vi.mocked(invoke).mockResolvedValue(PREVIEW);

  renderWithTheme(<OkfInteropBar />);
  fireEvent.click(screen.getByRole("button", { name: /import bundle/i }));
  await screen.findByText(/project x/i);
  fireEvent.click(screen.getByRole("button", { name: /cancel/i }));

  expect(screen.queryByText(/project x/i)).not.toBeInTheDocument();
  const applyCalls = vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === "okf_import_apply_cmd");
  expect(applyCalls).toHaveLength(0);
});

test("errors surface as notice", async () => {
  vi.mocked(open).mockResolvedValue("/tmp/bad.zip");
  vi.mocked(invoke).mockRejectedValue("Not an OKF bundle: no entities found.");
  renderWithTheme(<OkfInteropBar />);
  fireEvent.click(screen.getByRole("button", { name: /import bundle/i }));
  expect(await screen.findByText(/not an okf bundle/i)).toBeInTheDocument();
});
