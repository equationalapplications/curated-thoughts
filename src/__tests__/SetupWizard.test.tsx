import { render, screen, fireEvent } from "@testing-library/react";
import { SetupWizard } from "../components/setup/SetupWizard";

vi.mock("../hooks/usePrivacyMode", () => ({
  usePrivacyMode: () => ({
    mode: "strict",
    chosen: false,
    needs_migration_disclosure: false,
    ephemeral_disclosure_acknowledged: false,
    loading: false,
    setMode: vi.fn().mockResolvedValue(undefined),
  }),
}));

test("renders welcome step on mount", () => {
  render(<SetupWizard onComplete={vi.fn()} />);
  expect(screen.getByText(/your second brain/i)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /get started/i })).toBeInTheDocument();
});

test("clicking Get Started advances to privacy step", () => {
  render(<SetupWizard onComplete={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: /get started/i }));
  expect(screen.getByText(/Choose your privacy posture/i)).toBeInTheDocument();
});

test("privacy step continues to Fastembed step", async () => {
  render(<SetupWizard onComplete={vi.fn()} initialStep={1} />);
  fireEvent.click(screen.getByRole("button", { name: /continue/i }));
  expect(await screen.findByText(/setting up local search engine/i)).toBeInTheDocument();
});

test("calls onComplete when done step button clicked", () => {
  const onComplete = vi.fn();
  render(<SetupWizard onComplete={onComplete} initialStep={4} />);
  fireEvent.click(screen.getByRole("button", { name: /open my brain/i }));
  expect(onComplete).toHaveBeenCalledTimes(1);
});
