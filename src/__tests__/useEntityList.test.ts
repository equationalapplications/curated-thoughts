import { renderHook, waitFor, act } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { useEntityList } from "../hooks/useEntityList";

const ENTITY = {
  id: "ent_1",
  name: "Alpha",
  entity_type: "project",
  summary_snippet: "",
  fact_count: 2,
  open_task_count: 0,
  created_at: 100,
  updated_at: 200,
};

test("loads entities sorted by updated_desc and refreshes on demand", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "list_entities_cmd") return Promise.resolve([ENTITY]);
    return Promise.resolve(null);
  });

  const { result } = renderHook(() => useEntityList());
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.entities).toEqual([ENTITY]);
  expect(invoke).toHaveBeenCalledWith("list_entities_cmd", { sort: "updated_desc", filter: {} });

  const second = { ...ENTITY, id: "ent_2", name: "Beta" };
  vi.mocked(invoke).mockImplementation((cmd: string) =>
    cmd === "list_entities_cmd" ? Promise.resolve([ENTITY, second]) : Promise.resolve(null),
  );
  await act(() => result.current.refresh());
  expect(result.current.entities).toHaveLength(2);
});

test("captures load errors", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) =>
    cmd === "list_entities_cmd" ? Promise.reject(new Error("db locked")) : Promise.resolve(null),
  );
  const { result } = renderHook(() => useEntityList());
  await waitFor(() => expect(result.current.error).toBe("db locked"));
});
