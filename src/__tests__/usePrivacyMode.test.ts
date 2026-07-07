import { renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { usePrivacyMode } from "../hooks/usePrivacyMode";

const connectedState = {
  mode: "connected" as const,
  chosen: true,
  needs_migration_disclosure: false,
  ephemeral_disclosure_acknowledged: true,
};

test("loads mode from get_privacy_mode invoke", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === "get_privacy_mode") return Promise.resolve(connectedState);
    return Promise.resolve(null);
  });

  const { result } = renderHook(() => usePrivacyMode());
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.mode).toBe("connected");
});

test("setMode calls set_privacy_mode invoke", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "get_privacy_mode") {
      return Promise.resolve({
        mode: "strict",
        chosen: true,
        needs_migration_disclosure: false,
        ephemeral_disclosure_acknowledged: false,
      });
    }
    if (cmd === "set_privacy_mode") {
      return Promise.resolve({
        disconnected_bridge: false,
        state: { ...connectedState, mode: args?.mode },
      });
    }
    return Promise.resolve(null);
  });

  const { result } = renderHook(() => usePrivacyMode());
  await waitFor(() => expect(result.current.loading).toBe(false));
  await result.current.setMode("connected");
  expect(invoke).toHaveBeenCalledWith("set_privacy_mode", { mode: "connected" });
  await waitFor(() => expect(result.current.mode).toBe("connected"));
});
