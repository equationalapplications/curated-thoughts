import { render, screen, waitFor } from "@testing-library/react";
import { vi, describe, it, expect, beforeEach } from "vitest";
import { StepFastembed } from "../components/setup/StepFastembed";

vi.mock("../lib/tauri", () => ({
  initFastembed: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../lib/events", () => ({
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
});
