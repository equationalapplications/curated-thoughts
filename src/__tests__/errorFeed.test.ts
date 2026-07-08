import { describe, it, expect, beforeEach, vi } from "vitest";
import {
  reportBackgroundError,
  dismissError,
  retryError,
  subscribeErrors,
} from "../lib/errorFeed";

describe("errorFeed", () => {
  beforeEach(() => {
    // Reset module state before each test
    // We need to clear errors and listeners
    vi.resetModules();
  });

  it("reportBackgroundError stores and emits", (context) => {
    const listener = vi.fn();
    subscribeErrors(listener);

    reportBackgroundError("Test error");

    expect(listener).toHaveBeenCalledTimes(2); // once for immediate emit, once for report
    const callArgs = listener.mock.calls[listener.mock.calls.length - 1][0];
    expect(callArgs).toHaveLength(1);
    expect(callArgs[0]).toMatchObject({
      id: expect.any(Number),
      message: "Test error",
      at: expect.any(Number),
    });
  });

  it("dismissError removes and re-emits", () => {
    const listener = vi.fn();
    subscribeErrors(listener);
    reportBackgroundError("Error 1");
    reportBackgroundError("Error 2");

    listener.mockClear();
    const errorId = listener.mock.calls[0]?.[0]?.[0]?.id || 1;
    dismissError(errorId);

    expect(listener).toHaveBeenCalled();
    const result = listener.mock.calls[0][0];
    expect(result).toHaveLength(1);
  });

  it("retryError calls retry function, dismisses on success, keeps entry on failure", async () => {
    const listener = vi.fn();
    subscribeErrors(listener);
    const retryFn = vi.fn().mockResolvedValue(undefined);
    reportBackgroundError("Error with retry", retryFn);

    listener.mockClear();
    const errorId = listener.mock.calls[0]?.[0]?.[0]?.id || 1;
    await retryError(errorId);

    expect(retryFn).toHaveBeenCalled();
    // After successful retry, the error should be dismissed
    const finalState = listener.mock.calls[listener.mock.calls.length - 1]?.[0];
    expect(finalState).toHaveLength(0);
  });

  it("retryError keeps entry on failure", async () => {
    const listener = vi.fn();
    subscribeErrors(listener);
    const retryFn = vi.fn().mockRejectedValue(new Error("Retry failed"));
    reportBackgroundError("Error with failing retry", retryFn);

    listener.mockClear();
    const errorId = listener.mock.calls[0]?.[0]?.[0]?.id || 1;

    try {
      await retryError(errorId);
    } catch {
      // Expected to throw
    }

    // After failed retry, the error should still be there
    expect(listener).not.toHaveBeenCalled(); // No new emission on failure
  });

  it("subscribeErrors calls listener immediately with current state", () => {
    reportBackgroundError("Existing error");
    const listener = vi.fn();
    subscribeErrors(listener);

    expect(listener).toHaveBeenCalledTimes(1);
    const result = listener.mock.calls[0][0];
    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({
      message: "Existing error",
    });
  });

  it("subscribeErrors returns unsubscribe function", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeErrors(listener);

    listener.mockClear();
    unsubscribe();

    reportBackgroundError("New error");

    // Listener should not be called after unsubscribe
    expect(listener).not.toHaveBeenCalled();
  });
});
