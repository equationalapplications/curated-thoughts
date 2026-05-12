import { render, screen, fireEvent } from "@testing-library/react";
import { SetupWizard } from "../components/setup/SetupWizard";

test("renders welcome step on mount", () => {
  render(<SetupWizard onComplete={vi.fn()} />);
  expect(screen.getByText(/your second brain/i)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /get started/i })).toBeInTheDocument();
});

test("clicking Get Started advances to Ollama step", () => {
  render(<SetupWizard onComplete={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: /get started/i }));
  expect(screen.getByText(/set up ai model/i)).toBeInTheDocument();
});

test("calls onComplete when done step button clicked", () => {
  const onComplete = vi.fn();
  render(<SetupWizard onComplete={onComplete} initialStep={2} />);
  fireEvent.click(screen.getByRole("button", { name: /open my brain/i }));
  expect(onComplete).toHaveBeenCalledTimes(1);
});
