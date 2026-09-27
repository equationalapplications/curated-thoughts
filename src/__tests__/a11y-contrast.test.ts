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
    [t, "--success", "--bg", 4.5, `${name} success/ok text`],
    [t, "--warning", "--bg", 4.5, `${name} warning text`],
    // --disabled-* is the inactive-control treatment (see `button:disabled`).
    // The label must clear 4.5:1 on its OWN fill and on the two surfaces a
    // control can be stacked on: --bg (a button on a page) and --elev-2 (a
    // button on a card). The border is floored at 2:1, not 3:1 — SC 1.4.11
    // exempts inactive components, and this border is intentionally fainter
    // than --outline-var; the dashed pattern carries the state, not the
    // contrast. 2:1 still guarantees the control's extent is perceivable.
    [t, "--disabled-fg", "--disabled-bg", 4.5, `${name} disabled-button label on its own fill`],
    [t, "--disabled-fg", "--bg", 4.5, `${name} disabled-button label on a page surface`],
    [t, "--disabled-fg", "--elev-2", 4.5, `${name} disabled-button label on a card`],
    [t, "--disabled-border", "--disabled-bg", 2, `${name} disabled-button boundary on its own fill`],
    [t, "--disabled-border", "--bg", 2, `${name} disabled-button boundary on a page surface`],
    [t, "--disabled-border", "--elev-2", 2, `${name} disabled-button boundary on a card`],
    // --separator is the chrome-hairline token (rail/sidebar/panel edges).
    // It is NOT an interactive-control boundary — those use --outline-var at
    // 3:1 above — so the floor here is only "actually visible", which is
    // still a real gate: a separator that matches its background is invisible.
    [t, "--separator", "--bg", 1.15, `${name} chrome hairlines`],
    [t, "--separator", "--elev-1", 1.1, `${name} chrome hairlines on rail/sidebar`],
    [t, "--separator", "--elev-2", 1.1, `${name} chrome hairlines on cards`],
  );
}

describe("a11y: token contrast (WCAG 2.2 AA)", () => {
  it("every fg/bg pair meets its WCAG threshold", () => {
    // Resolve token NAMES to their parsed hex values before the math. Passing
    // the name strings into contrast() would yield NaN, and NaN < min is
    // always false — the gate would silently pass on a failing palette
    // (CodeRabbit finding, fixed 2026-09-01).
    const failures = PAIRS.filter(([theme, fgName, bgName, min]) => {
        const fg = theme[fgName];
        const bg = theme[bgName];
        if (fg === undefined || bg === undefined) return true;
        // Hex-format + finite-ratio guards: a malformed or shorthand token
        // would make luminance() return NaN, and NaN < min is always false —
        // the gate must fail loudly instead (CodeRabbit round-3).
        const HEX = /^#[0-9a-fA-F]{6}$/;
        if (!HEX.test(fg) || !HEX.test(bg)) return true;
        const ratio = contrast(fg, bg);
        return !Number.isFinite(ratio) || ratio < min;
      })
      .map(([theme, fgName, bgName, min, why]) => {
        const fg = theme[fgName];
        const bg = theme[bgName];
        const ratio = fg !== undefined && bg !== undefined ? contrast(fg, bg).toFixed(2) : "undefined";
        return `${why}: ${fgName}(${fg ?? "missing"}) on ${bgName}(${bg ?? "missing"}) = ${ratio} < ${min}`;
      });
    expect(failures).toEqual([]);
  });

  it("retuned tokens have the approved values", () => {
    // Light keeps the warm-paper palette it has always had; the dark theme
    // was retuned to a neutral slate set (2026-09-26 visual pass).
    expect(light["--outline"]).toBe("#6b5e50");
    expect(light["--outline-var"]).toBe("#94826e");
    expect(light["--separator"]).toBe("#e0d3c4");
    expect(dark["--outline"]).toBe("#8b95a3");
    expect(dark["--outline-var"]).toBe("#6f7a87");
    expect(dark["--separator"]).toBe("#262c33");
    expect(dark["--success"]).toBe("#4bbf73");
    expect(dark["--warning"]).toBe("#e0a458");
    expect(light["--disabled-bg"]).toBe("#faf5f0");
    expect(light["--disabled-border"]).toBe("#ab9a84");
    expect(light["--disabled-fg"]).toBe("#6b5e50");
    expect(dark["--disabled-bg"]).toBe("#171a1f");
    expect(dark["--disabled-border"]).toBe("#525c68");
    expect(dark["--disabled-fg"]).toBe("#8b95a3");
    expect(indexCss).toContain("--focus-ring: 2px solid var(--primary);");
  });

  it("both themes define the same token set", () => {
    // A token defined in :root but not in [data-theme="dark"] (or vice versa)
    // silently falls back to the light value at runtime. jsdom has no cascade,
    // so assert the two blocks agree on names here.
    const names = (t: Record<string, string>) => Object.keys(t).sort();
    expect(names(dark)).toEqual(names(light));
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

describe("a11y: inline SVG data-URIs are well-formed", () => {
  // The <select> caret shipped as `<polyline .../></polyline>` — self-closed
  // AND closed. That is invalid XML, and an invalid data-URI SVG fails to
  // render *silently*: the caret vanished from every <select> in the app with
  // no console error and no failing test. Nothing but a structural assertion
  // catches this class of bug, so assert the structure.
  const dataUris = [...`${indexCss}\n${appCss}`.matchAll(/data:image\/svg\+xml,([^"')]+)/g)].map(
    (m) => decodeURIComponent(m[1]),
  );

  it("finds the select caret URIs in both themes", () => {
    expect(dataUris.length).toBeGreaterThanOrEqual(2);
  });

  it.each(dataUris)("no element is both self-closed and closed: %s", (svg) => {
    for (const m of svg.matchAll(/<([a-zA-Z][\w:-]*)\b[^>]*\/>/g)) {
      expect(
        svg.includes(`</${m[1]}>`),
        `<${m[1]}> is self-closed and also has a closing tag — invalid XML, ` +
          `so the image silently fails to render`,
      ).toBe(false);
    }
  });

  it.each(dataUris)("every opening tag is closed or self-closed: %s", (svg) => {
    for (const m of svg.matchAll(/<([a-zA-Z][\w:-]*)\b[^>]*?(\/?)>/g)) {
      if (m[2] === "/") continue;
      expect(svg.includes(`</${m[1]}>`), `<${m[1]}> is never closed`).toBe(true);
    }
  });
});

describe("a11y: disabled controls stay legible AND look inactive", () => {
  // Two failure modes have to be excluded at once:
  //   1. `button:disabled { opacity: 0.5 }` composited the near-black
  //      --on-primary label toward the dark background and made the Tasks
  //      "Create" button unreadable.
  //   2. Fixing that with a plain --elev-2 fill + --on-surface-var label made
  //      a disabled primary button pixel-identical to an enabled secondary
  //      one, so it read as clickable.
  // The treatment that satisfies both is structural (dashed border, fill that
  // recedes to the page, muted-but-AA label, not-allowed cursor), and it must
  // live in ONE place — a per-variant `:disabled` copy outranks the base rule
  // and is how the drift comes back.
  const baseRule = (() => {
    const start = indexCss.indexOf("button:disabled {");
    expect(start).toBeGreaterThan(-1);
    return indexCss.slice(start, indexCss.indexOf("}", start));
  })();

  it("does not dim disabled buttons with opacity", () => {
    expect(baseRule).not.toMatch(/opacity:\s*0?\.\d/);
  });

  it("reads as inactive without costing legibility", () => {
    expect(baseRule).toContain("var(--disabled-bg)");
    expect(baseRule).toContain("var(--disabled-fg)");
    expect(baseRule).toContain("border-style: dashed");
    expect(baseRule).toContain("cursor: not-allowed");
  });

  it("the inactive treatment is not shared with the ENABLED secondary button", () => {
    // The regression that made "Create" look clickable. If a disabled button
    // ever falls back to the enabled pair, the two are indistinguishable.
    const enabled = indexCss.slice(
      indexCss.indexOf("button {"),
      indexCss.indexOf("}", indexCss.indexOf("button {")),
    );
    expect(enabled).toContain("var(--elev-2)");
    expect(baseRule).not.toContain("var(--elev-2)");
    expect(baseRule).not.toContain("var(--outline-var)");
  });

  it("no component re-declares its own disabled treatment", () => {
    // Every `:disabled` rule in the stylesheets must be the three base rules
    // or the documented borderless-rail exception, so the treatment cannot
    // drift per variant. A per-variant copy outranks the base rule (class +
    // pseudo-class beats element + pseudo-class), which is exactly how the
    // disabled "Create" button drifted back to looking enabled.
    const ALLOWED = new Set([
      "button:disabled",
      "input:disabled",
      "select:disabled",
      "textarea:disabled",
      ".mode-rail-history-btn:disabled",
      "input:disabled,select:disabled,textarea:disabled",
    ]);
    // Comments are stripped first: several of them name a `:disabled` rule
    // in prose, and a text mention is not a rule. `:not(:disabled)` hover
    // guards are excluded — they are the opposite of a disabled treatment.
    const css = `${indexCss}\n${appCss}`.replace(/\/\*[\s\S]*?\*\//g, "");
    const found = [...css.matchAll(/(^|\})\s*([^{}]*:disabled[^{}]*)\{/g)]
      .map((m) => m[2].split(",").map((s) => s.trim()).join(",").replace(/\s+/g, " "))
      .filter((sel) => !ALLOWED.has(sel) && !sel.includes(":not("));
    expect(found).toEqual([]);
  });

  it("hides the native number-input stepper instead of showing a light one", () => {
    expect(indexCss).toContain("input[type=\"number\"]::-webkit-inner-spin-button");
    expect(indexCss).toContain("input[type=\"number\"]::-webkit-outer-spin-button");
  });
});
