import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { vi, describe, it, expect, beforeEach } from "vitest";
import { StepModel } from "../components/setup/StepModel";

vi.mock("../lib/tauri", () => ({
  downloadSidecarEngine: vi.fn().mockResolvedValue(undefined),
  downloadModelWeights: vi.fn().mockResolvedValue(undefined),
  updateProvider: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../lib/events", () => ({
  onGgufDownloadProgress: vi.fn().mockResolvedValue(() => {}),
  onSidecarDownloadProgress: vi.fn().mockResolvedValue(() => {}),
  onProviderReady: vi.fn().mockImplementation((cb: () => void) => {
     cb();
     return Promise.resolve(() => {});
  }),
  onProviderError: vi.fn().mockResolvedValue(() => {}),
}));

import {
  downloadSidecarEngine,
  downloadModelWeights,
  updateProvider,
} from "../lib/tauri";

describe("StepModel", () => {
  const onNext = vi.fn();

  beforeEach(() => {
    vi.resetAllMocks();
    (downloadSidecarEngine as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
    (downloadModelWeights as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
    (updateProvider as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
  });

  it("renders two choices on load", () => {
    render(<StepModel onNext={onNext} />);
    expect(screen.getByText(/Auto-Install/i)).toBeInTheDocument();
    expect(screen.getByText(/Skip/i)).toBeInTheDocument();
  });

  it("save with blank URL calls updateProvider with unconfigured", async () => {
    render(<StepModel onNext={onNext} />);
    fireEvent.click(screen.getByText(/Skip/i));
    fireEvent.click(screen.getByRole("button", { name: /Save & continue/i }));
    await waitFor(() =>
      expect(updateProvider).toHaveBeenCalledWith(
        expect.objectContaining({ provider: "unconfigured" }),
      ),
    );
    await waitFor(() => expect(onNext).toHaveBeenCalledOnce());
  });

  it("skip with external URL calls updateProvider with external", async () => {
    render(<StepModel onNext={onNext} />);
    fireEvent.click(screen.getByText(/Skip/i));
    const input = screen.getByPlaceholderText(/http:\/\/localhost/i);
    fireEvent.change(input, { target: { value: "http://localhost:11434/v1" } });
    fireEvent.click(screen.getByRole("button", { name: /Save & continue/i }));
    await waitFor(() =>
      expect(updateProvider).toHaveBeenCalledWith(
        expect.objectContaining({
          provider: "external",
          external_url: "http://localhost:11434/v1",
        }),
      ),
    );
  });

  it("auto-install shows error when checksums are not configured", async () => {
    render(<StepModel onNext={onNext} />);
    fireEvent.click(screen.getByText(/Auto-Install/i));
    await waitFor(() => {
      expect(screen.getByText(/Auto-install is unavailable/i)).toBeInTheDocument();
    });
    expect(downloadSidecarEngine).not.toHaveBeenCalled();
    expect(downloadModelWeights).not.toHaveBeenCalled();
  });
});
