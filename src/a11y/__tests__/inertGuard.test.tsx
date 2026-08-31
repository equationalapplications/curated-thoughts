import { render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { applyInertGuard } from "../inertGuard";

/**
 * inertGuard contract (Task 4 brief):
 * - applies `inert` + `aria-hidden="true"` to background content when
 *   `HTMLElement.prototype.inert` is supported
 * - falls back to `aria-hidden` only + `pointer-events: none` style when
 *   not (WebView2 gap)
 * - fully removes both on release (restores previous values exactly)
 * - never touches the dialog itself, its ancestors, `.a11y-announcer`,
 *   or `.skip-link`
 * - handles dialogs nested deep in the tree: siblings at EVERY level
 *   along the ancestor chain get guarded (CT mounts everything under
 *   a single #root, so a naive body-children scan would guard nothing)
 */

function mountTree(): { dialog: HTMLElement; cleanup: () => void } {
  const container = document.createElement("div");
  container.id = "root";
  const appRoot = document.createElement("div");
  appRoot.className = "app-root";
  const background = document.createElement("div");
  background.className = "app-body";
  const announcer = document.createElement("div");
  announcer.className = "a11y-announcer";
  const skip = document.createElement("a");
  skip.className = "skip-link";
  const dialog = document.createElement("aside");
  dialog.className = "peek-panel";
  appRoot.append(background, announcer, skip, dialog);
  container.append(appRoot);
  document.body.append(container);
  return { dialog, cleanup: () => container.remove() };
}

describe("applyInertGuard", () => {
  let tree: ReturnType<typeof mountTree>;
  const supportsInert = "inert" in HTMLElement.prototype;

  beforeEach(() => {
    tree = mountTree();
  });

  afterEach(() => {
    tree.cleanup();
  });

  it("guards background siblings at every ancestor level, never the dialog or its ancestors", () => {
    const release = applyInertGuard(tree.dialog);
    const background = document.querySelector<HTMLElement>(".app-body")!;

    expect(background.getAttribute("aria-hidden")).toBe("true");
    if (supportsInert) {
      expect(background.inert).toBe(true);
    } else {
      expect(background.style.pointerEvents).toBe("none");
    }
    // dialog untouched
    expect(tree.dialog.hasAttribute("aria-hidden")).toBe(false);
    if (supportsInert) expect(tree.dialog.inert).toBe(false);
    // exempt classes untouched
    const announcer = document.querySelector<HTMLElement>(".a11y-announcer")!;
    expect(announcer.hasAttribute("aria-hidden")).toBe(false);
    const skip = document.querySelector<HTMLElement>(".skip-link")!;
    expect(skip.hasAttribute("aria-hidden")).toBe(false);
    // ancestors of the dialog untouched (they contain it)
    const appRoot = document.querySelector<HTMLElement>(".app-root")!;
    expect(appRoot.hasAttribute("aria-hidden")).toBe(false);

    release();
  });

  it("restores exactly what it changed on release", () => {
    const background = document.querySelector<HTMLElement>(".app-body")!;
    background.setAttribute("aria-hidden", "false"); // pre-existing value
    const release = applyInertGuard(tree.dialog);
    expect(background.getAttribute("aria-hidden")).toBe("true");
    release();
    expect(background.getAttribute("aria-hidden")).toBe("false");
    if (supportsInert) expect(background.inert).toBe(false);
    else expect(background.style.pointerEvents).toBe("");
  });

  it("supports an explicit root narrower than document.body", () => {
    const appRoot = document.querySelector<HTMLElement>(".app-root")!;
    const release = applyInertGuard(tree.dialog, appRoot);
    const background = document.querySelector<HTMLElement>(".app-body")!;
    expect(background.getAttribute("aria-hidden")).toBe("true");
    release();
    expect(background.getAttribute("aria-hidden")).toBeNull();
  });

  it("falls back to aria-hidden + pointer-events when inert is unavailable", () => {
    const proto = HTMLElement.prototype as unknown as Record<string, unknown>;
    const hadInert = supportsInert;
    let restored = false;
    if (hadInert) {
      try {
        delete proto.inert;
      } catch {
        restored = true; // not configurable; skip simulation
      }
    }
    try {
      if (!(hadInert && restored)) {
        const background = document.querySelector<HTMLElement>(".app-body")!;
        const release = applyInertGuard(tree.dialog);
        expect(background.getAttribute("aria-hidden")).toBe("true");
        expect(background.style.pointerEvents).toBe("none");
        release();
        expect(background.style.pointerEvents).toBe("");
        expect(background.getAttribute("aria-hidden")).toBeNull();
      }
    } finally {
      if (hadInert && !restored && !("inert" in HTMLElement.prototype)) {
        Object.defineProperty(HTMLElement.prototype, "inert", {
          configurable: true,
          enumerable: true,
          get(this: HTMLElement) {
            return (this as unknown as { _inert?: boolean })._inert ?? false;
          },
          set(this: HTMLElement, v: boolean) {
            (this as unknown as { _inert?: boolean })._inert = v;
          },
        });
      }
    }
  });

  it("renders inside a React tree (smoke with testing-library)", () => {
    const { unmount } = render(
      <div className="app-root">
        <div className="app-body">background</div>
        <aside className="peek-panel" role="dialog" aria-modal="true" />
      </div>,
    );
    const dialog = document.querySelector(".peek-panel")!;
    const release = applyInertGuard(dialog);
    const background = document.querySelector<HTMLElement>(".app-body")!;
    expect(background.getAttribute("aria-hidden")).toBe("true");
    release();
    expect(background.hasAttribute("aria-hidden")).toBe(false);
    unmount();
  });
});
