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
    // pull silently never started. The pull must proceed, the panel must say
    // progress is unavailable, and guardListen must log exactly one warn.
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    // Create the rejection lazily per call so it is handled in the same tick
    // guardListen attaches its catch (an eagerly-rejected promise would fire
    // a spurious unhandledRejection before the component ever subscribes).
    (onPullProgress as ReturnType<typeof vi.fn>).mockImplementation(
      () => Promise.reject(new Error("listen boom")),
    );

    render(<ModelPanel />);
    fireEvent.change(screen.getByPlaceholderText(/Model name/i), {
      target: { value: "llama3.2:3b" },
    });
    fireEvent.click(screen.getByRole("button", { name: /Pull model/i }));

    await waitFor(() => expect(pullModel).toHaveBeenCalledWith("llama3.2:3b"));
    // The pull-proceeds behavior itself: a rejected registration must not
    // abort the pull, and guardListen logs exactly one warn.
    // (The hint is rendered only during the pulling phase — fast-review fix —
    // and this mocked pull resolves in the same tick as registration, so the
    // hint can flash past before any waitFor poll sees it. StepOllama's test
    // pins the hint visually by holding the pull open.)
    await waitFor(() =>
      expect(
        screen.getByText(/Model pulled successfully\./),
      ).toBeInTheDocument(),
    );
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
