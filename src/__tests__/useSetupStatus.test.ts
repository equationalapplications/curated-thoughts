import { renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { useSetupStatus } from "../hooks/useSetupStatus";

const mockInvoke = invoke as ReturnType<typeof vi.fn>;

test("needsSetup true when vault path is null", async () => {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "get_vault_path") return Promise.resolve(null);
    if (cmd === "check_ollama") return Promise.resolve({ installed: true, running: true, models: ["llama3.2:3b"] });
    return Promise.resolve(null);
  });
  const { result } = renderHook(() => useSetupStatus());
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.needsSetup).toBe(true);
});

test("needsSetup false when vault set and Ollama running", async () => {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "get_vault_path") return Promise.resolve("/Users/test/vault");
    if (cmd === "check_ollama") return Promise.resolve({ installed: true, running: true, models: ["llama3.2:3b"] });
    return Promise.resolve(null);
  });
  const { result } = renderHook(() => useSetupStatus());
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.needsSetup).toBe(false);
});

test("needsSetup true when Ollama not installed", async () => {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "get_vault_path") return Promise.resolve("/Users/test/vault");
    if (cmd === "check_ollama") return Promise.resolve({ installed: false, running: false, models: [] });
    return Promise.resolve(null);
  });
  const { result } = renderHook(() => useSetupStatus());
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.needsSetup).toBe(true);
});
