import { render, screen, fireEvent } from "@testing-library/react";
import { vi, describe, it, expect, beforeEach } from "vitest";

const mockEvents = vi.hoisted(() =>
  Array.from({ length: 50 }, (_, i) => ({
    id: `event-${i}`,
    kind: "synthesized" as const,
    summary: `Event ${i}`,
    entity_id: i % 2 === 0 ? `entity-${i}` : null,
    entity_name: i % 2 === 0 ? `Entity ${i}` : null,
    doc_path: i % 2 === 1 ? `/doc/${i}.md` : null,
    raw_type: "test_event",
    client: null,
    created_at_ms: Date.now() - i * 1000,
  })),
);

vi.mock("../hooks/useTimeline", () => ({
  useTimeline: vi.fn().mockReturnValue({
    events: mockEvents,
    error: null,
    refresh: vi.fn(),
  }),
}));

vi.mock("../hooks/useErrorFeed", () => ({
  useErrorFeed: vi.fn().mockReturnValue({
    errors: [],
    dismiss: vi.fn(),
    retry: vi.fn(),
  }),
}));

import { ActivityFeedPanel } from "../components/shell/ActivityFeedPanel";
import { useTimeline } from "../hooks/useTimeline";
import { useErrorFeed } from "../hooks/useErrorFeed";
import type { BackgroundError } from "../lib/errorFeed";

describe("ActivityFeedPanel", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    (useTimeline as ReturnType<typeof vi.fn>).mockReturnValue({
      events: mockEvents,
      error: null,
      refresh: vi.fn(),
    });
    (useErrorFeed as ReturnType<typeof vi.fn>).mockReturnValue({
      errors: [],
      dismiss: vi.fn(),
      retry: vi.fn(),
    });
  });

  it("renders_50_events_from_useTimeline_mock", () => {
    const onNavigate = vi.fn();
    const onClose = vi.fn();

    render(
      <ActivityFeedPanel
        isOpen={true}
        onClose={onClose}
        onNavigate={onNavigate}
      />
    );

    // Verify the panel is visible
    expect(screen.getByRole("dialog", { name: "Activity feed" })).toBeInTheDocument();

    // Verify useTimeline was called with limit: 50
    expect(useTimeline).toHaveBeenCalledWith({ limit: 50 });

    // Verify title is present
    expect(screen.getByText("Activity")).toBeInTheDocument();
  });

  it("open_full_timeline_button_navigates_and_closes", () => {
    const onNavigate = vi.fn();
    const onClose = vi.fn();

    render(
      <ActivityFeedPanel
        isOpen={true}
        onClose={onClose}
        onNavigate={onNavigate}
      />
    );

    // Find and click the "Open full Timeline" button
    const button = screen.getByRole("button", { name: "Open full Timeline" });
    fireEvent.click(button);

    // Verify navigation was called with timeline mode
    expect(onNavigate).toHaveBeenCalledWith({ mode: "timeline" });

    // Verify close was called
    expect(onClose).toHaveBeenCalled();
  });

  it("still_renders_without_error_entries", () => {
    const onNavigate = vi.fn();
    const onClose = vi.fn();

    (useTimeline as ReturnType<typeof vi.fn>).mockReturnValue({
      events: mockEvents,
      error: null,
      refresh: vi.fn(),
    });

    render(
      <ActivityFeedPanel
        isOpen={true}
        onClose={onClose}
        onNavigate={onNavigate}
      />
    );

    // Verify the panel renders without errors
    expect(screen.getByRole("dialog", { name: "Activity feed" })).toBeInTheDocument();

    // Verify no error banner is shown when error is null
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("does_not_render_when_closed", () => {
    const onNavigate = vi.fn();
    const onClose = vi.fn();

    const { container } = render(
      <ActivityFeedPanel
        isOpen={false}
        onClose={onClose}
        onNavigate={onNavigate}
      />
    );

    // Verify the panel is not visible
    expect(screen.queryByRole("dialog", { name: "Activity feed" })).not.toBeInTheDocument();
    expect(container.firstChild).toBeNull();
  });

  it("shows_error_banner_when_error_is_set", () => {
    const onNavigate = vi.fn();
    const onClose = vi.fn();
    const errorMessage = "Timeline is temporarily unavailable.";

    (useTimeline as ReturnType<typeof vi.fn>).mockReturnValue({
      events: [],
      error: errorMessage,
      refresh: vi.fn(),
    });

    render(
      <ActivityFeedPanel
        isOpen={true}
        onClose={onClose}
        onNavigate={onNavigate}
      />
    );

    // Verify error banner is shown
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(screen.getByText(errorMessage)).toBeInTheDocument();
  });

  it("close_button_calls_onClose", () => {
    const onNavigate = vi.fn();
    const onClose = vi.fn();

    render(
      <ActivityFeedPanel
        isOpen={true}
        onClose={onClose}
        onNavigate={onNavigate}
      />
    );

    // Find and click the close button (×)
    const closeButton = screen.getByRole("button", { name: "Close" });
    fireEvent.click(closeButton);

    // Verify onClose was called
    expect(onClose).toHaveBeenCalled();
  });

  it("backdrop_click_calls_onClose", () => {
    const onNavigate = vi.fn();
    const onClose = vi.fn();

    render(
      <ActivityFeedPanel
        isOpen={true}
        onClose={onClose}
        onNavigate={onNavigate}
      />
    );

    // Find and click the backdrop
    const backdrop = screen.getByRole("button", { name: "Close activity feed" });
    fireEvent.click(backdrop);

    // Verify onClose was called
    expect(onClose).toHaveBeenCalled();
  });

  it("renders_error_entries_with_retry_and_dismiss_buttons", () => {
    const onNavigate = vi.fn();
    const onClose = vi.fn();
    const mockRetry = vi.fn();
    const mockDismiss = vi.fn();

    const errors: BackgroundError[] = [
      {
        id: 1,
        message: "Test error 1",
        at: Date.now(),
        retry: mockRetry,
      },
      {
        id: 2,
        message: "Test error 2 (no retry)",
        at: Date.now(),
      },
    ];

    (useErrorFeed as ReturnType<typeof vi.fn>).mockReturnValue({
      errors,
      dismiss: mockDismiss,
      retry: vi.fn(),
    });

    render(
      <ActivityFeedPanel
        isOpen={true}
        onClose={onClose}
        onNavigate={onNavigate}
      />
    );

    // Verify error messages are rendered
    expect(screen.getByText("Test error 1")).toBeInTheDocument();
    expect(screen.getByText("Test error 2 (no retry)")).toBeInTheDocument();

    // Verify Retry button is shown for first error
    const retryButtons = screen.getAllByRole("button", { name: "Retry" });
    expect(retryButtons).toHaveLength(1);

    // Verify Dismiss buttons are shown for both errors
    const dismissButtons = screen.getAllByRole("button", { name: "Dismiss" });
    expect(dismissButtons).toHaveLength(2);
  });

  it("dismiss_button_calls_dismiss_function", () => {
    const onNavigate = vi.fn();
    const onClose = vi.fn();
    const mockDismiss = vi.fn();

    const errors: BackgroundError[] = [
      {
        id: 1,
        message: "Test error",
        at: Date.now(),
      },
    ];

    (useErrorFeed as ReturnType<typeof vi.fn>).mockReturnValue({
      errors,
      dismiss: mockDismiss,
      retry: vi.fn(),
    });

    render(
      <ActivityFeedPanel
        isOpen={true}
        onClose={onClose}
        onNavigate={onNavigate}
      />
    );

    // Click dismiss button
    const dismissButton = screen.getByRole("button", { name: "Dismiss" });
    fireEvent.click(dismissButton);

    // Verify dismiss was called with error id
    expect(mockDismiss).toHaveBeenCalledWith(1);
  });

  it("retry_button_calls_retry_function", () => {
    const onNavigate = vi.fn();
    const onClose = vi.fn();
    const mockRetry = vi.fn();

    const errors: BackgroundError[] = [
      {
        id: 1,
        message: "Test error with retry",
        at: Date.now(),
        retry: vi.fn(),
      },
    ];

    (useErrorFeed as ReturnType<typeof vi.fn>).mockReturnValue({
      errors,
      dismiss: vi.fn(),
      retry: mockRetry,
    });

    render(
      <ActivityFeedPanel
        isOpen={true}
        onClose={onClose}
        onNavigate={onNavigate}
      />
    );

    // Click retry button
    const retryButton = screen.getByRole("button", { name: "Retry" });
    fireEvent.click(retryButton);

    // Verify retry was called with error id
    expect(mockRetry).toHaveBeenCalledWith(1);
  });

  it("uses_errors_prop_when_provided", () => {
    const onNavigate = vi.fn();
    const onClose = vi.fn();
    const mockDismiss = vi.fn();

    const errorsProp: BackgroundError[] = [
      {
        id: 99,
        message: "Prop error",
        at: Date.now(),
      },
    ];

    render(
      <ActivityFeedPanel
        isOpen={true}
        onClose={onClose}
        onNavigate={onNavigate}
        errors={errorsProp}
      />
    );

    // Verify prop error is rendered instead of hook errors
    expect(screen.getByText("Prop error")).toBeInTheDocument();
  });
});
