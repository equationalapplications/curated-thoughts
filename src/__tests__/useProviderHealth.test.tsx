import { renderHook, waitFor } from "@testing-library/react";
import { vi, describe, it, expect, beforeEach, afterEach } from "vitest";
import { useProviderHealth } from "../hooks/useProviderHealth";

vi.mock("../lib/tauri", () => ({
  getProviderConfig: vi.fn(),
}));

vi.mock("../lib/events", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/events")>()),
  onProviderLoading: vi.fn(),
  onProviderReady: vi.fn(),
  onProviderError: vi.fn(),
  onEmbedInitProgress: vi.fn(),
  onEmbedInitDone: vi.fn(),
  onEmbedInitError: vi.fn(),
}));

import { getProviderConfig } from "../lib/tauri";
import {
  onProviderLoading,
  onProviderReady,
  onProviderError,
  onEmbedInitProgress,
  onEmbedInitDone,
  onEmbedInitError,
} from "../lib/events";

type MockedEvent = ReturnType<typeof vi.fn>;

describe("useProviderHealth", () => {
  let warnSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    vi.resetAllMocks();
    (getProviderConfig as MockedEvent).mockResolvedValue({
      generation: { provider: "sidecar" },
      embedding: { provider: "fastembed" },
    });
    warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
  });

  afterEach(() => {
    warnSpy.mockRestore();
  });

  it("unlistens every subscription on unmount even when one listen() rejects", async () => {
    const unlistenReady = vi.fn();
    const unlistenError = vi.fn();
    const unlistenProgress = vi.fn();
    const unlistenDone = vi.fn();
    const unlistenEmbedError = vi.fn();

    // One subscription rejects; the rest resolve with distinct unlisten fns.
    (onProviderLoading as MockedEvent).mockImplementation(() =>
      Promise.reject(new Error("listen boom")),
    );
    (onProviderReady as MockedEvent).mockResolvedValue(unlistenReady);
    (onProviderError as MockedEvent).mockResolvedValue(unlistenError);
    (onEmbedInitProgress as MockedEvent).mockResolvedValue(unlistenProgress);
    (onEmbedInitDone as MockedEvent).mockResolvedValue(unlistenDone);
    (onEmbedInitError as MockedEvent).mockResolvedValue(unlistenEmbedError);

    const { unmount } = renderHook(() => useProviderHealth());

    // The failed registration surfaces as exactly one guarded warn.
    await waitFor(() => {
      const warns = warnSpy.mock.calls.map((c: unknown[]) => String(c[0]));
      expect(
        warns.filter((m: string) =>
          m.includes("[events] listen failed (provider-loading)"),
        ).length,
      ).toBe(1);
    });

    unmount();

    // Every surviving subscription is unlistened — the old Promise.all
    // shape dropped the pending promises when one listen() rejected.
    await waitFor(() => {
      expect(unlistenReady).toHaveBeenCalledOnce();
      expect(unlistenError).toHaveBeenCalledOnce();
      expect(unlistenProgress).toHaveBeenCalledOnce();
      expect(unlistenDone).toHaveBeenCalledOnce();
      expect(unlistenEmbedError).toHaveBeenCalledOnce();
    });
  });
});
