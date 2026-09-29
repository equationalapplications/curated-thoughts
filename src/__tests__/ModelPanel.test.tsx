import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { vi, describe, it, expect, beforeEach } from "vitest";
import { ModelPanel } from "../components/settings/ModelPanel";

vi.mock("../lib/tauri", () => ({
  listLocalModels: vi.fn().mockResolvedValue(["qwen3:0.6b"]),
  pullModel: vi.fn().mockResolvedValue(undefined),
  getRecommendedModel: vi.fn().mockResolvedValue("llama3.2:3b"),
}));

vi.mock("../lib/events", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/events")>()),
  onPullProgress: vi.fn(),
}));

import { getRecommendedModel, listLocalModels, pullModel } from "../lib/tauri";
import { onPullProgress } from "../lib/events";

describe("ModelPanel", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    (listLocalModels as ReturnType<typeof vi.fn>).mockResolvedValue([
      "qwen3:0.6b",
    ]);
    (pullModel as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
    (getRecommendedModel as ReturnType<typeof vi.fn>).mockResolvedValue(
      "llama3.2:3b",
    );
  });

  it("still pulls when the progress listener rejects (degraded display only)", async () => {
    // Regression (#236, review M4): the old shape `await onPullProgress(...)`
    // threw before pullModel ran when registration rejected, so the user's
    // pull silently never started. The pull must proceed, guardListen must
    // log exactly one warn, and the "Progress unavailable" hint must show
    // while pulling and disappear once the pull ends.
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    // Create the rejection lazily per call so it is handled in the same tick
    // guardListen attaches its catch (an eagerly-rejected promise would fire
    // a spurious unhandledRejection before the component ever subscribes).
    (onPullProgress as ReturnType<typeof vi.fn>).mockImplementation(
      () => Promise.reject(new Error("listen boom")),
    );
    // Hold the pull open so the pulling phase (and its hint) is observable,
    // mirroring the StepOllama test.
    let resolvePull: () => void = () => {};
    (pullModel as ReturnType<typeof vi.fn>).mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          resolvePull = resolve;
        }),
    );

    render(<ModelPanel />);
    fireEvent.change(screen.getByPlaceholderText(/Model name/i), {
      target: { value: "llama3.2:3b" },
    });
    fireEvent.click(screen.getByRole("button", { name: /Pull model/i }));

    await waitFor(() => expect(pullModel).toHaveBeenCalledWith("llama3.2:3b"));
    // While pulling: the hint is visible (registration rejected → degraded
    // display, but the pull continues).
    await waitFor(() =>
      expect(
        screen.getByText(/Progress unavailable — pull continuing\./),
      ).toBeInTheDocument(),
    );
    resolvePull();
    await waitFor(() =>
      expect(
        screen.getByText(/Model pulled successfully\./),
      ).toBeInTheDocument(),
    );
    // After the pull ends the hint must NOT linger.
    expect(
      screen.queryByText(/Progress unavailable — pull continuing\./),
    ).not.toBeInTheDocument();
    const warns = warnSpy.mock.calls.map((c: unknown[]) => String(c[0]));
    expect(
      warns.filter((m: string) =>
        m.includes("[events] listen failed (ollama-pull-progress)"),
      ).length,
    ).toBe(1);
  });
});
