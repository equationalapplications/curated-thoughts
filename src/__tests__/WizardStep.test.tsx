import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { WizardStep } from "../components/setup/WizardStep";

describe("WizardStep", () => {
  it("renders title and subtitle as a labelled region", () => {
    render(
      <WizardStep title="Pick" subtitle="Outcome" onNext={vi.fn()}>
        <p>body</p>
      </WizardStep>,
    );
    const region = screen.getByRole("region", { name: /pick/i });
    expect(region).toBeInTheDocument();
    expect(screen.getByText("Outcome")).toBeInTheDocument();
    expect(screen.getByText("body")).toBeInTheDocument();
  });

  it("hides Back when onBack is undefined and hides Next when onNext is undefined", () => {
    const { rerender } = render(<WizardStep title="A">body</WizardStep>);
    expect(screen.queryByRole("button", { name: /back/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /continue/i })).not.toBeInTheDocument();
    rerender(<WizardStep title="B" onBack={vi.fn()} onNext={vi.fn()}>body</WizardStep>);
    expect(screen.getByRole("button", { name: /back/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /continue/i })).toBeInTheDocument();
  });

  it("renders Skip when onSkip is provided with the default label", () => {
    render(
      <WizardStep title="C" onNext={vi.fn()} onSkip={vi.fn()}>
        body
      </WizardStep>,
    );
    expect(screen.getByRole("button", { name: /skip — take me to the app/i })).toBeInTheDocument();
  });

  it("uses custom skip label when provided", () => {
    render(
      <WizardStep title="D" onNext={vi.fn()} onSkip={vi.fn()} skipLabel="Skip ahead">
        body
      </WizardStep>,
    );
    expect(screen.getByRole("button", { name: /skip ahead/i })).toBeInTheDocument();
  });

  it("disables Next when nextDisabled and shows spinner when isLoading", () => {
    render(
      <WizardStep title="E" onNext={vi.fn()} nextDisabled isLoading>
        body
      </WizardStep>,
    );
    expect(screen.getByRole("button", { name: /continue/i })).toBeDisabled();
    expect(screen.getByTestId("wizard-step-spinner")).toBeInTheDocument();
  });

  it("fires onBack, onNext, and onSkip when clicked", () => {
    const onBack = vi.fn(), onNext = vi.fn(), onSkip = vi.fn();
    render(
      <WizardStep title="F" onBack={onBack} onNext={onNext} onSkip={onSkip}>
        body
      </WizardStep>,
    );
    fireEvent.click(screen.getByRole("button", { name: /back/i }));
    fireEvent.click(screen.getByRole("button", { name: /continue/i }));
    fireEvent.click(screen.getByRole("button", { name: /skip/i }));
    expect(onBack).toHaveBeenCalledOnce();
    expect(onNext).toHaveBeenCalledOnce();
    expect(onSkip).toHaveBeenCalledOnce();
  });
});
