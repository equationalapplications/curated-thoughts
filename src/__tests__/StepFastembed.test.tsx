import { render, screen, waitFor } from "@testing-library/react";
import { vi, describe, it, expect, beforeEach, afterEach } from "vitest";
import { StepFastembed } from "../components/setup/StepFastembed";

vi.mock("../lib/tauri", () => ({
  initFastembed: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../lib/events", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/events")>()),
  onEmbedInitDone: vi.fn(),
  onEmbedInitError: vi.fn(),
}));

import { initFastembed } from "../lib/tauri";
import { onEmbedInitDone, onEmbedInitError } from "../lib/events";

describe("StepFastembed", () => {
  const onNext = vi.fn();

  beforeEach(() => {
    vi.resetAllMocks();
    (onEmbedInitDone as ReturnType<typeof vi.fn>).mockImplementation(
      (cb: () => void) => {
        cb();
        return Promise.resolve(() => {});
      },
    );
    (onEmbedInitError as ReturnType<typeof vi.fn>).mockResolvedValue(() => {});
    (initFastembed as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows spinner while initializing", () => {
    render(<StepFastembed onNext={onNext} />);
    expect(screen.getByText(/Set up local search/i)).toBeInTheDocument();
  });

  it("calls onNext when embed-init-done fires", async () => {
    render(<StepFastembed onNext={onNext} />);
    await waitFor(() => expect(onNext).toHaveBeenCalledOnce());
  });

  it("shows error when embed-init-error fires", async () => {
    (onEmbedInitError as ReturnType<typeof vi.fn>).mockImplementation(
      (cb: (payload: { message: string }) => void) => {
        cb({ message: "download failed" });
        return Promise.resolve(() => {});
      },
    );
    render(<StepFastembed onNext={onNext} />);
    await waitFor(() => expect(screen.getByText(/download failed/i)).toBeInTheDocument());
  });

  it("removes listeners that resolve after the step unmounts", async () => {
    const unlistenDone = vi.fn();
    const unlistenError = vi.fn();
    let resolveDone!: (fn: () => void) => void;
    let resolveError!: (fn: () => void) => void;
    (onEmbedInitDone as ReturnType<typeof vi.fn>).mockReturnValue(
      new Promise((r) => (resolveDone = r)),
    );
    (onEmbedInitError as ReturnType<typeof vi.fn>).mockReturnValue(
      new Promise((r) => (resolveError = r)),
    );

    const { unmount } = render(<StepFastembed onNext={onNext} />);
    unmount();
    resolveDone(unlistenDone);
    resolveError(unlistenError);

    await waitFor(() => {
      expect(unlistenDone).toHaveBeenCalledOnce();
      expect(unlistenError).toHaveBeenCalledOnce();
    });
  });

  it("still calls initFastembed when a subscription rejects", async () => {
    // Regression (#236): the old Promise.all shape rejected as soon as one
    // subscription failed, skipping initFastembed entirely — the model never
    // initialized and the wizard stalled. With allSettled, a rejected
    // subscription must degrade to one guarded warn while init proceeds.
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    // mockImplementation (not mockReturnValue): an eagerly-created rejected
    // promise can fire unhandledrejection before the component subscribes;
    // lazy creation matches the ModelPanel/StepOllama tests.
    (onEmbedInitDone as ReturnType<typeof vi.fn>).mockImplementation(() =>
      Promise.reject(new Error("listen boom")),
    );
    (onEmbedInitError as ReturnType<typeof vi.fn>).mockResolvedValue(() => {});

    render(<StepFastembed onNext={onNext} />);

    await waitFor(() => expect(initFastembed).toHaveBeenCalledOnce());
    const warns = warnSpy.mock.calls.map((c: unknown[]) => String(c[0]));
    expect(
      warns.filter((m: string) =>
        m.includes("[events] listen failed (embed-init-done)"),
      ).length,
    ).toBe(1);
  });
});
