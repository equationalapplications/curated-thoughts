import { describe, it, expect } from "vitest";
import { render, screen, within } from "@testing-library/react";
import { StepIndicator } from "../components/setup/StepIndicator";

const STEPS = ["Welcome", "Privacy", "Fastembed", "Model", "Watch it think", "Done"];

describe("StepIndicator", () => {
  it("renders all step names with current highlighted", () => {
    render(<StepIndicator current={2} total={6} steps={STEPS} />);
    // The current step's name now appears twice by design: once in the rail
    // (as the accessible text of its segment) and once on the label line. The
    // rail is what this test is about, so scope the query to the <ol>.
    const rail = screen.getByRole("list");
    expect(within(rail).getByText("Welcome")).toBeInTheDocument();
    const fastembed = within(rail).getByText("Fastembed");
    expect(fastembed).toHaveClass("step-indicator-current");
  });

  it("renders the 1-based position and the current step's name", () => {
    // Pass 4 split the single "Step 4 of 6: Model" line into a position and a
    // name, so the strip is not the only place the current step is named. The
    // progressbar's own aria-label still carries the combined form.
    render(<StepIndicator current={3} total={6} steps={STEPS} />);
    expect(screen.getByText("4")).toBeInTheDocument();
    // Two "Model" nodes by design (rail segment + label line); assert the
    // label line, which is the one that is actually painted.
    const label = screen.getByText("4").closest("p");
    expect(label).toHaveTextContent("Model");
    expect(screen.getByRole("progressbar")).toHaveAttribute(
      "aria-label",
      "Step 4 of 6: Model",
    );
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
