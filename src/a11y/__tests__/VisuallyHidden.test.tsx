import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { VisuallyHidden } from "../VisuallyHidden";

describe("VisuallyHidden", () => {
  it("renders its children into the DOM", () => {
    render(<VisuallyHidden>Screen-reader only label</VisuallyHidden>);
    expect(screen.getByText("Screen-reader only label")).toBeInTheDocument();
  });

  it("keeps content in the accessibility tree while visually clipped", () => {
    const { container } = render(<VisuallyHidden>clipped</VisuallyHidden>);
    const span = container.firstElementChild;
    expect(span).not.toBeNull();
    expect(span!.tagName).toBe("SPAN");
    const style = (span as HTMLElement).style;
    // Visual clip contract (screen-reader-safe visually-hidden pattern). jsdom
    // serializes `clip: rect(0 0 0 0)` as "rect(0px)", so compare normalized.
    const normalized = (value: string) => value.replace(/\s+/g, " ").trim();
    expect(style.position).toBe("absolute");
    expect(style.width).toBe("1px");
    expect(style.height).toBe("1px");
    expect(style.overflow).toBe("hidden");
    expect(normalized(style.clip)).toMatch(/^rect\((0px?\s*)+\)$/);
    expect(style.clipPath).toBe("inset(50%)");
    expect(style.whiteSpace).toBe("nowrap");
    expect(style.margin).toBe("-1px");
  });

  it("does not hide content from assistive tech (no display:none / visibility:hidden)", () => {
    const { container } = render(<VisuallyHidden>still announced</VisuallyHidden>);
    const style = (container.firstElementChild as HTMLElement).style;
    expect(style.display).not.toBe("none");
    expect(style.visibility).not.toBe("hidden");
  });
});
