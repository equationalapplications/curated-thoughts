// Contrast is verified here at the token level because jsdom axe runs have no
// computed colors (color-contrast disabled there). This test parses the real
// index.css and computes WCAG 2.x relative luminance directly — no new deps.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const ROOT = join(__dirname, "..", "..");
const indexCss = readFileSync(join(ROOT, "src", "index.css"), "utf8");
const appCss = readFileSync(join(ROOT, "src", "App.css"), "utf8");

function luminance(hex: string): number {
  const h = hex.replace("#", "");
  const [r, g, b] = [0, 2, 4].map((i) => {
    const c = parseInt(h.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrast(fg: string, bg: string): number {
  const [a, b] = [luminance(fg), luminance(bg)].sort((x, y) => y - x);
  return (a + 0.05) / (b + 0.05);
}

function tokens(selector: string): Record<string, string> {
  const start = indexCss.indexOf(selector);
  expect(start, `selector ${selector} present`).toBeGreaterThan(-1);
  const open = indexCss.indexOf("{", start);
  const close = indexCss.indexOf("}", open);
  const out: Record<string, string> = {};
  for (const m of indexCss.slice(open, close).matchAll(/--([\w-]+):\s*(#[0-9a-fA-F]{6})/g)) {
    out[`--${m[1]}`] = m[2];
  }
  return out;
}

const light = tokens(":root");
const dark = tokens('[data-theme="dark"]');

// [fg, bg, minRatio, why]
const PAIRS: Array<[Record<string, string>, string, string, number, string]> = [];
for (const [name, t] of [["light", light], ["dark", dark]] as const) {
  PAIRS.push(
    [t, "--on-surface", "--bg", 4.5, `${name} body text`],
    [t, "--on-surface-var", "--bg", 4.5, `${name} secondary text`],
    [t, "--on-surface-var", "--elev-2", 4.5, `${name} secondary text on cards`],
    [t, "--on-surface", "--elev-3", 4.5, `${name} text on highest elevation`],
    [t, "--outline", "--bg", 4.5, `${name} outline-as-text (8+ call sites)`],
    [t, "--outline", "--elev-2", 4.5, `${name} outline-as-text on cards`],
    [t, "--outline", "--elev-3", 4.5, `${name} outline-as-text on highest elevation`],
    [t, "--error", "--bg", 4.5, `${name} error text`],
    [t, "--primary", "--bg", 4.5, `${name} primary text/buttons`],
    [t, "--secondary", "--bg", 4.5, `${name} secondary accents`],
    [t, "--on-primary", "--primary", 4.5, `${name} text on primary`],
    [t, "--on-primary-cont", "--primary-container", 4.5, `${name} text on primary container`],
    [t, "--outline-var", "--bg", 3, `${name} UI boundaries (SC 1.4.11)`],
    [t, "--outline-var", "--elev-3", 3, `${name} UI boundaries on highest elevation`],
    [t, "--primary", "--surface-variant", 3, `${name} primary icons on variant surface`],
    [t, "--primary", "--elev-3", 3, `${name} primary icons on highest elevation`],
  );
}

describe("a11y: token contrast (WCAG 2.2 AA)", () => {
  it("every fg/bg pair meets its WCAG threshold", () => {
    const failures = PAIRS.filter(([, fg, bg, min]) => fg === undefined || bg === undefined || contrast(fg, bg) < min)
      .map(([, fg, bg, min, why]) => `${why}: ${fg} on ${bg} = ${fg && bg ? contrast(fg, bg).toFixed(2) : "undefined"} < ${min}`);
    expect(failures).toEqual([]);
  });

  it("retuned tokens have the approved values", () => {
    expect(light["--outline"]).toBe("#6b5e50");
    expect(light["--outline-var"]).toBe("#94826e");
    expect(dark["--outline"]).toBe("#a89a89");
    expect(dark["--outline-var"]).toBe("#8d7e6a");
    expect(indexCss).toContain("--focus-ring: 3px solid var(--primary);");
  });

  it("no orphaned outline: none remains (focus-visible strategy instead)", () => {
    expect(indexCss.includes("outline: none")).toBe(false);
    expect(appCss.includes("outline: none")).toBe(false);
  });

  it("prefers-reduced-motion override is present and is the LAST rule (so it wins)", () => {
    const reducedStart = indexCss.indexOf("prefers-reduced-motion");
    expect(reducedStart).toBeGreaterThan(-1);
    expect(reducedStart).toBeGreaterThan(indexCss.lastIndexOf("transition:"));
    expect(reducedStart).toBeGreaterThan(indexCss.lastIndexOf("animation:"));
    const tail = indexCss.slice(reducedStart);
    expect(tail).toContain("animation-duration: 0.01ms !important");
    expect(tail).toContain("animation-iteration-count: 1 !important");
    expect(tail).toContain("transition-duration: 0.01ms !important");
    expect(tail).toContain("scroll-behavior: auto !important");
  });

  it(":focus-visible ring rule is present", () => {
    expect(indexCss).toMatch(/:focus-visible\s*\{[^}]*outline:\s*var\(--focus-ring\)/);
  });
});
