import { screen } from "@testing-library/react";
import { ActivityFeedPanel } from "../components/shell/ActivityFeedPanel";
import { renderWithTheme } from "./test-utils";
import type { BackgroundError } from "../lib/errorFeed";

test("renders placeholder text when open", () => {
  renderWithTheme(
    <ActivityFeedPanel isOpen={true} onClose={vi.fn()} onNavigate={vi.fn()} />
  );
  expect(
    screen.getByText(/Live librarian events will appear here/i)
  ).toBeInTheDocument();
});

test("does not render when closed", () => {
  const { container } = renderWithTheme(
    <ActivityFeedPanel isOpen={false} onClose={vi.fn()} onNavigate={vi.fn()} />
  );
  expect(container.querySelector(".activity-panel")).not.toBeInTheDocument();
});

test("calls onClose when backdrop is clicked", () => {
  const onClose = vi.fn();
  renderWithTheme(
    <ActivityFeedPanel isOpen={true} onClose={onClose} onNavigate={vi.fn()} />
  );
  screen.getByLabelText("Close activity feed").click();
  expect(onClose).toHaveBeenCalled();
});

test("calls onClose when close button is clicked", () => {
  const onClose = vi.fn();
  renderWithTheme(
    <ActivityFeedPanel isOpen={true} onClose={onClose} onNavigate={vi.fn()} />
  );
  screen.getByLabelText("Close").click();
  expect(onClose).toHaveBeenCalled();
});

test("renders errors when provided", () => {
  const errors: BackgroundError[] = [
    { id: 1, message: "Error 1", at: Date.now() },
    { id: 2, message: "Error 2", at: Date.now(), retry: vi.fn() },
  ];
  renderWithTheme(
    <ActivityFeedPanel
      isOpen={true}
      onClose={vi.fn()}
      onNavigate={vi.fn()}
      errors={errors}
    />
  );
  expect(screen.getByText("Error 1")).toBeInTheDocument();
  expect(screen.getByText("Error 2")).toBeInTheDocument();
});

test("calls retry when retry button is clicked", () => {
  const retry = vi.fn().mockResolvedValue(undefined);
  const errors: BackgroundError[] = [
    { id: 1, message: "Retry me", at: Date.now(), retry },
  ];
  renderWithTheme(
    <ActivityFeedPanel
      isOpen={true}
      onClose={vi.fn()}
      onNavigate={vi.fn()}
      errors={errors}
    />
  );
  screen.getByText("Retry").click();
  expect(retry).toHaveBeenCalled();
});

test("calls dismiss when dismiss button is clicked", () => {
  const onDismiss = vi.fn();
  const errors: BackgroundError[] = [
    { id: 1, message: "Dismiss me", at: Date.now() },
  ];
  renderWithTheme(
    <ActivityFeedPanel
      isOpen={true}
      onClose={vi.fn()}
      onNavigate={vi.fn()}
      errors={errors}
      onDismiss={onDismiss}
    />
  );
  screen.getByLabelText("Dismiss").click();
  expect(onDismiss).toHaveBeenCalledWith(1);
});

test("uses_errors_prop_when_provided", () => {
  const onNavigate = vi.fn();
  const onClose = vi.fn();

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
