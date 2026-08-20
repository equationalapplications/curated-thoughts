import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, act } from "@testing-library/react";
import { StepWatchItThink } from "../components/setup/StepWatchItThink";
import { open } from "@tauri-apps/plugin-dialog";
import { ingestDocument } from "../lib/tauri";
import {
  onIngestProgress,
  onIngestProposalReady,
  onIngestError,
} from "../lib/events";

vi.mock("../lib/tauri", () => ({
  ingestDocument: vi.fn(),
}));
vi.mock("../lib/events", () => ({
  onIngestProgress: vi.fn(),
  onIngestProposalReady: vi.fn(),
  onIngestError: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

type ProgressCb = (p: { phase: string; path: string }) => void;
type ReadyCb = (p: { path: string; proposalId: string | null }) => void;
type ErrorCb = (p: { message: string }) => void;

describe("StepWatchItThink", () => {
  let progressCb: ProgressCb;
  let readyCb: ReadyCb;
  let errorCb: ErrorCb;

  beforeEach(() => {
    vi.resetAllMocks();
    // shouldAdvanceTime keeps React 19's act()/RTL waitFor scheduling alive
    // while still allowing vi.advanceTimersByTime for the stall watchdog.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    progressCb = vi.fn();
    readyCb = vi.fn();
    errorCb = vi.fn();
    (open as ReturnType<typeof vi.fn>).mockResolvedValue("/vault/doc.md");
    (ingestDocument as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
    (onIngestProgress as ReturnType<typeof vi.fn>).mockImplementation((cb: ProgressCb) => {
      progressCb = cb;
      return Promise.resolve(() => {});
    });
    (onIngestProposalReady as ReturnType<typeof vi.fn>).mockImplementation((cb: ReadyCb) => {
      readyCb = cb;
      return Promise.resolve(() => {});
    });
    (onIngestError as ReturnType<typeof vi.fn>).mockImplementation((cb: ErrorCb) => {
      errorCb = cb;
      return Promise.resolve(() => {});
    });
  });

  it("renders the file-picker button on mount", () => {
    render(<StepWatchItThink onSkip={vi.fn()} onRouteToReview={vi.fn()} />);
    expect(screen.getByRole("button", { name: /choose a document to ingest/i })).toBeInTheDocument();
  });

  it("calls onSkip when the Skip button is clicked", () => {
    const onSkip = vi.fn();
    render(<StepWatchItThink onSkip={onSkip} onRouteToReview={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /skip — take me to the app/i }));
    expect(onSkip).toHaveBeenCalledOnce();
  });

  it("stays in idle when the file picker is cancelled", async () => {
    (open as ReturnType<typeof vi.fn>).mockResolvedValue(null);
    render(<StepWatchItThink onSkip={vi.fn()} onRouteToReview={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /choose a document to ingest/i }));
    await waitFor(() => expect(open).toHaveBeenCalled());
    expect(ingestDocument).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: /choose a document to ingest/i })).toBeInTheDocument();
  });

  it("calls ingestDocument and shows chunking status after file pick", async () => {
    render(<StepWatchItThink onSkip={vi.fn()} onRouteToReview={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /choose a document to ingest/i }));
    await waitFor(() => expect(ingestDocument).toHaveBeenCalledWith("/vault/doc.md"));
    expect(screen.getByText(/chunking/i)).toBeInTheDocument();
  });

  it("updates status on each onIngestProgress event", async () => {
    render(<StepWatchItThink onSkip={vi.fn()} onRouteToReview={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /choose a document to ingest/i }));
    await waitFor(() => expect(ingestDocument).toHaveBeenCalled());
    act(() => progressCb({ phase: "embedding", path: "/vault/doc.md" }));
    expect(screen.getByText(/embedding/i)).toBeInTheDocument();
  });

  it("auto-routes to Review when proposal-ready fires with a proposalId", async () => {
    const onRouteToReview = vi.fn();
    render(<StepWatchItThink onSkip={vi.fn()} onRouteToReview={onRouteToReview} />);
    fireEvent.click(screen.getByRole("button", { name: /choose a document to ingest/i }));
    await waitFor(() => expect(ingestDocument).toHaveBeenCalled());
    act(() => readyCb({ path: "/vault/doc.md", proposalId: "prop_42" }));
    expect(onRouteToReview).toHaveBeenCalledWith("prop_42");
  });

  it("renders inline error panel and Try-again button when ingest-error fires", async () => {
    render(<StepWatchItThink onSkip={vi.fn()} onRouteToReview={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /choose a document to ingest/i }));
    await waitFor(() => expect(ingestDocument).toHaveBeenCalled());
    act(() => errorCb({ message: "boom" }));
    expect(screen.getByText(/boom/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /try again/i })).toBeInTheDocument();
  });

  it("flips to stalled after 60s without progress", async () => {
    render(<StepWatchItThink onSkip={vi.fn()} onRouteToReview={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /choose a document to ingest/i }));
    await waitFor(() => expect(ingestDocument).toHaveBeenCalled());
    act(() => {
      vi.advanceTimersByTime(61_000);
    });
    expect(screen.getByText(/still working/i)).toBeInTheDocument();
  });
});
