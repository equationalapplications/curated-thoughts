import { describe, it, expect, beforeEach, vi } from "vitest";
import {
  reportBackgroundError,
  dismissError,
  retryError,
  subscribeErrors,
  __resetErrorFeed,
} from "../lib/errorFeed";

describe("errorFeed", () => {
  beforeEach(() => {
    // Reset module state before each test
    __resetErrorFeed();
  });

  it("reportBackgroundError stores and emits", () => {
    const listener = vi.fn();
    subscribeErrors(listener);

    reportBackgroundError("Test error");

    // Once for subscription initial state, once for report
    expect(listener).toHaveBeenCalledTimes(2);
    const lastCall = listener.mock.calls[1][0];
    expect(lastCall).toHaveLength(1);
    expect(lastCall[0]).toMatchObject({
      id: expect.any(Number),
      message: "Test error",
      at: expect.any(Number),
    });
  });

  it("dismissError removes and re-emits", () => {
    reportBackgroundError("Error 1");
    reportBackgroundError("Error 2");

    const listener = vi.fn();
    subscribeErrors(listener);

    // Get the current errors to find an ID
    let errorId = 1;
    listener.mock.calls[0][0].forEach((err) => {
      if (err.message === "Error 1") {
        errorId = err.id;
      }
    });

    listener.mockClear();
    dismissError(errorId);

    expect(listener).toHaveBeenCalledTimes(1);
    const result = listener.mock.calls[0][0];
    expect(result).toHaveLength(1);
    expect(result[0].message).toBe("Error 2");
  });

  it("retryError calls retry function, dismisses on success", async () => {
    const retryFn = vi.fn().mockResolvedValue(undefined);
    reportBackgroundError("Error with retry", retryFn);

    const listener = vi.fn();
    subscribeErrors(listener);

    const errorId = listener.mock.calls[0][0][0].id;

    listener.mockClear();
    await retryError(errorId);

    expect(retryFn).toHaveBeenCalled();
    // After successful retry, error should be dismissed
    const finalCall = listener.mock.calls[listener.mock.calls.length - 1][0];
    expect(finalCall).toHaveLength(0);
  });

  it("retryError keeps entry on failure", async () => {
    const retryFn = vi.fn().mockRejectedValue(new Error("Retry failed"));
    reportBackgroundError("Error with failing retry", retryFn);

    const listener = vi.fn();
    subscribeErrors(listener);

    const errorId = listener.mock.calls[0][0][0].id;

    listener.mockClear();

    try {
      await retryError(errorId);
    } catch {
      // Expected to throw
    }

    // After failed retry, no new emission should occur
    expect(listener).not.toHaveBeenCalled();
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
