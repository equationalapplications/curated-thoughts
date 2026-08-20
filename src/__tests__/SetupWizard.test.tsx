import { render, screen, fireEvent } from "@testing-library/react";
vi.mock("../components/setup/StepWatchItThink", () => ({
  StepWatchItThink: () => <div data-testid="step-watch-it-think">Watch it think</div>,
}));
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

test("renders welcome step with vault path and Continue button", () => {
  render(<SetupWizard onComplete={vi.fn()} vaultPath="/notes" />);
  expect(screen.getByText("Where is your vault?")).toBeInTheDocument();
  expect(screen.getByText("/notes")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /continue/i })).toBeInTheDocument();
});

test("clicking Continue advances to privacy step", () => {
  render(<SetupWizard onComplete={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: /continue/i }));
  expect(screen.getByText(/Choose your privacy posture/i)).toBeInTheDocument();
});

test("privacy step continues to Fastembed step", async () => {
  render(<SetupWizard onComplete={vi.fn()} initialStep={1} />);
  fireEvent.click(screen.getByRole("button", { name: /continue/i }));
  expect(await screen.findByText(/set up local search/i)).toBeInTheDocument();
});

test("StepIndicator shows the current six-step position", () => {
  render(<SetupWizard onComplete={vi.fn()} initialStep={2} />);
  expect(screen.getByText("Step 3 of 6: Fastembed")).toBeInTheDocument();
});

test("Watch it think renders as step four", () => {
  render(<SetupWizard onComplete={vi.fn()} initialStep={4} />);
  expect(screen.getByTestId("step-watch-it-think")).toBeInTheDocument();
});

test("calls onComplete when Open My Brain is clicked", () => {
  const onComplete = vi.fn();
  render(<SetupWizard onComplete={onComplete} initialStep={5} />);
  fireEvent.click(screen.getByRole("button", { name: /open my brain/i }));
  expect(onComplete).toHaveBeenCalledTimes(1);
});
