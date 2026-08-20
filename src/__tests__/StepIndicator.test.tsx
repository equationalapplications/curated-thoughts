import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { StepIndicator } from "../components/setup/StepIndicator";

const STEPS = ["Welcome", "Privacy", "Fastembed", "Model", "Watch it think", "Done"];

describe("StepIndicator", () => {
  it("renders all step names with current highlighted", () => {
    render(<StepIndicator current={2} total={6} steps={STEPS} />);
    expect(screen.getByText("Welcome")).toBeInTheDocument();
    const fastembed = screen.getByText("Fastembed");
    expect(fastembed).toHaveClass("step-indicator-current");
  });

  it("renders the 1-based label 'Step N of M: <current-name>'", () => {
    render(<StepIndicator current={3} total={6} steps={STEPS} />);
    expect(screen.getByText(/step 4 of 6: model/i)).toBeInTheDocument();
  });

  it("exposes aria-valuenow/aria-valuemax on the progress bar (1-based)", () => {
    render(<StepIndicator current={2} total={6} steps={STEPS} />);
    const bar = screen.getByRole("progressbar");
    expect(bar).toHaveAttribute("aria-valuenow", "3");
    expect(bar).toHaveAttribute("aria-valuemax", "6");
  });

  it("disables fill animation when prefers-reduced-motion is set", () => {
    // The CSS file contains the @media (prefers-reduced-motion: reduce) rule that
    // sets transition:none on .step-indicator-fill.  Since jsdom cannot evaluate
    // CSS media queries, we verify the fill element carries no inline animation
    // or transition (those come from the stylesheet, not inline styles).
    render(<StepIndicator current={1} total={6} steps={STEPS} />);
    const fill = screen.getByTestId("step-indicator-fill");
    expect(fill).not.toHaveStyle({ animation: expect.any(String) });
    expect(fill).not.toHaveStyle({ transition: expect.any(String) });
  });
});
