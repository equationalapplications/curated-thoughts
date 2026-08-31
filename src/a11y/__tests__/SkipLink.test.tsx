import { render, screen } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { SkipLink } from "../SkipLink";

const ROOT = join(__dirname, "..", "..", "..");
const indexCss = readFileSync(join(ROOT, "src", "index.css"), "utf8");

describe("SkipLink", () => {
  it("renders an anchor to #<targetId>", () => {
    render(<SkipLink targetId="main-content" />);
    const link = screen.getByRole("link", { name: "Skip to main content" });
    expect(link).toHaveAttribute("href", "#main-content");
  });

  it("accepts a custom label", () => {
    render(<SkipLink targetId="main-content" label="Skip to editor" />);
    expect(screen.getByRole("link", { name: "Skip to editor" })).toHaveAttribute(
      "href",
      "#main-content",
    );
  });

  it("uses the .skip-link class (off-canvas CSS contract in index.css)", () => {
    render(<SkipLink targetId="main-content" />);
    expect(screen.getByRole("link")).toHaveClass("skip-link");
  });
});

describe("skip-link CSS contract (SC 2.4.1)", () => {
  it("is positioned off-canvas until :focus-visible, per index.css", () => {
    const block = indexCss.indexOf(".skip-link {");
    expect(block, ".skip-link block present in index.css").toBeGreaterThan(-1);
    const open = indexCss.indexOf("{", block);
    const close = indexCss.indexOf("}", open);
    const body = indexCss.slice(open, close);
    expect(body).toContain("position: fixed");
    expect(body).toContain("top: -64px");

    const focusRule = indexCss.indexOf(".skip-link:focus-visible { top: 12px; }");
    expect(focusRule, ":focus-visible brings link on-canvas").toBeGreaterThan(-1);
    expect(focusRule, "focus rule must come after the base block").toBeGreaterThan(block);
  });

  it("skip-link and announcer styles sit BEFORE the reduced-motion final rule", () => {
    const skipLink = indexCss.indexOf(".skip-link {");
    const announcer = indexCss.indexOf(".a11y-announcer {");
    const reducedMotion = indexCss.lastIndexOf("@media (prefers-reduced-motion: reduce)");
    expect(skipLink).toBeGreaterThan(-1);
    expect(announcer).toBeGreaterThan(skipLink);
    expect(reducedMotion).toBeGreaterThan(announcer);
    expect(indexCss.slice(reducedMotion)).toContain("animation-duration: 0.01ms !important");
  });
});
