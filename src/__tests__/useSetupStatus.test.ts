import { renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { useSetupStatus } from "../hooks/useSetupStatus";

const mockInvoke = invoke as ReturnType<typeof vi.fn>;

test("needsSetup true when vault path is null", async () => {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "get_vault_path") return Promise.resolve(null);
    if (cmd === "get_brain_dir") return Promise.resolve("/Users/test/.brain");
    if (cmd === "get_provider_config")
      return Promise.resolve({
        generation: { provider: "unconfigured", model_path: null, model_name: null, external_url: null, api_key: null },
        embedding: { provider: "fastembed", external_url: null },
      });
    return Promise.resolve(null);
  });
  const { result } = renderHook(() => useSetupStatus());
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.needsSetup).toBe(true);
});

test("needsSetup false when vault set and provider configured", async () => {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "get_vault_path") return Promise.resolve("/Users/test/vault");
    if (cmd === "get_brain_dir") return Promise.resolve("/Users/test/.brain");
    if (cmd === "get_provider_config")
      return Promise.resolve({
        generation: { provider: "sidecar", model_path: null, model_name: null, external_url: null, api_key: null },
        embedding: { provider: "fastembed", external_url: null },
      });
    return Promise.resolve(null);
  });
  const { result } = renderHook(() => useSetupStatus());
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.needsSetup).toBe(false);
});

test("needsSetup true when provider unconfigured", async () => {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "get_vault_path") return Promise.resolve("/Users/test/vault");
    if (cmd === "get_brain_dir") return Promise.resolve("/Users/test/.brain");
    if (cmd === "get_provider_config")
      return Promise.resolve({
        generation: { provider: "unconfigured", model_path: null, model_name: null, external_url: null, api_key: null },
        embedding: { provider: "fastembed", external_url: null },
      });
    return Promise.resolve(null);
  });
  const { result } = renderHook(() => useSetupStatus());
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.needsSetup).toBe(true);
});
