import { render, screen, fireEvent } from "@testing-library/react";
import { ModeRail } from "../components/shell/ModeRail";

test("renders Brain, Review, Library, and Settings buttons", () => {
  render(<ModeRail mode="brain" reviewCount={0} onModeChange={vi.fn()} />);
  expect(screen.getByRole("button", { name: "Brain" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Review" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Library" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Settings" })).toBeInTheDocument();
});

test("marks the active mode with aria-current", () => {
  render(<ModeRail mode="library" reviewCount={0} onModeChange={vi.fn()} />);
  expect(screen.getByRole("button", { name: "Library" })).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("button", { name: "Brain" })).not.toHaveAttribute("aria-current");
});

test("clicking a mode button calls onModeChange", () => {
  const onModeChange = vi.fn();
  render(<ModeRail mode="brain" reviewCount={0} onModeChange={onModeChange} />);
  fireEvent.click(screen.getByRole("button", { name: "Review" }));
  expect(onModeChange).toHaveBeenCalledWith("review");
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  expect(onModeChange).toHaveBeenCalledWith("settings");
});

test("shows review count badge only when non-zero", () => {
  const { rerender } = render(<ModeRail mode="brain" reviewCount={3} onModeChange={vi.fn()} />);
  expect(screen.getByText("3")).toBeInTheDocument();
  rerender(<ModeRail mode="brain" reviewCount={0} onModeChange={vi.fn()} />);
  expect(screen.queryByText("0")).not.toBeInTheDocument();
});
