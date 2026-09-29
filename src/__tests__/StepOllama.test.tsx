import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { vi, describe, it, expect, beforeEach } from "vitest";
import { StepOllama } from "../components/setup/StepOllama";

vi.mock("@tauri-apps/plugin-shell", () => ({
  open: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({
  checkOllama: vi.fn().mockResolvedValue({ installed: true, running: true }),
  getRecommendedModel: vi.fn().mockResolvedValue("llama3.2:3b"),
  pullModel: vi.fn().mockResolvedValue(undefined),
  startOllamaServer: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../lib/events", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/events")>()),
  onPullProgress: vi.fn(),
}));

import { checkOllama, getRecommendedModel, pullModel } from "../lib/tauri";
import { onPullProgress } from "../lib/events";

describe("StepOllama", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    (checkOllama as ReturnType<typeof vi.fn>).mockResolvedValue({
      installed: true,
      running: true,
    });
    (getRecommendedModel as ReturnType<typeof vi.fn>).mockResolvedValue(
      "llama3.2:3b",
    );
    (pullModel as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
  });

  it("still pulls when the progress listener rejects (degraded display only)", async () => {
    // Regression (#236, review M4): the old shape `await onPullProgress(...)`
    // threw before pullModel ran when registration rejected, so the wizard
    // stalled at "pulling" forever. The pull must proceed to ready, the hint
    // must render while pulling, and guardListen must log exactly one warn.
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    // Create the rejection lazily per call so it is handled in the same tick
    // guardListen attaches its catch (an eagerly-rejected promise would fire
    // a spurious unhandledRejection before the component ever subscribes).
    (onPullProgress as ReturnType<typeof vi.fn>).mockImplementation(
      () => Promise.reject(new Error("listen boom")),
    );
    // Hold the pull open so the pulling-phase hint is observable before the
    // phase flips to ready (the hint lives inside the pulling-phase JSX).
    let resolvePull!: () => void;
    (pullModel as ReturnType<typeof vi.fn>).mockReturnValue(
      new Promise<void>((r) => (resolvePull = () => r())),
    );

    render(<StepOllama onNext={vi.fn()} />);
    await waitFor(() =>
      expect(screen.getByLabelText(/Model name/i)).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: /Download & continue/i }));

    await waitFor(() => expect(pullModel).toHaveBeenCalledWith("llama3.2:3b"));
    await waitFor(() =>
      expect(
        screen.getByText(/Progress unavailable — pull continuing\./),
      ).toBeInTheDocument(),
    );
    resolvePull();
    await waitFor(() =>
      expect(screen.getByText(/Ollama is ready\./)).toBeInTheDocument(),
    );
    const warns = warnSpy.mock.calls.map((c: unknown[]) => String(c[0]));
    expect(
      warns.filter((m: string) =>
        m.includes("[events] listen failed (ollama-pull-progress)"),
      ).length,
    ).toBe(1);
  });
});
