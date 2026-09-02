/**
 * Background-content guard for modal dialogs (spec §Dialog semantics).
 *
 * Applies `inert` + `aria-hidden="true"` to everything outside the dialog so
 * screen-reader cursors and pointer focus cannot wander into the background.
 * Where `inert` is unsupported — WebView2 has a known gap with NVDA — the
 * guard falls back to `aria-hidden` plus `pointer-events: none`. The returned
 * function restores exactly what was changed, and nothing else.
 *
 * CT renders the whole app under a single `#root`, so the dialog's background
 * siblings live at EVERY level of the ancestor chain, not just at body level:
 * the guard walks up along the dialog's ancestor path and guards each level's
 * other children. The dialog itself, its ancestors, `.a11y-announcer`, and
 * `.skip-link` are never touched.
 */
export function applyInertGuard(
  dialog: Element,
  root: ParentNode = document.body,
  options: { allow?: (el: Element) => boolean } = {},
): () => void {
  interface Touched {
    el: HTMLElement;
    prevInert: boolean;
    prevAriaHidden: string | null;
    prevPointerEvents: string;
  }
  const touched: Touched[] = [];
  const supportsInert = "inert" in HTMLElement.prototype;

  const isExempt = (el: Element): boolean => {
    if (el === dialog || dialog.contains(el)) return true;
    if (el.contains(dialog)) return true; // ancestors contain the dialog
    const cls = el.classList;
    if (cls.contains("a11y-announcer") || cls.contains("skip-link")) return true;
    // Dialog-owned dismissal surfaces (e.g. click-outside backdrops) must
    // stay interactive — they are part of the dialog's own UX.
    if (options.allow?.(el)) return true;
    return false;
  };
  const guard = (el: Element): void => {
    if (isExempt(el)) return;
    const h = el as HTMLElement;
    touched.push({
      el: h,
      prevInert: supportsInert ? h.inert : false,
      prevAriaHidden: h.getAttribute("aria-hidden"),
      prevPointerEvents: h.style.pointerEvents,
    });
    if (supportsInert) {
      h.inert = true;
    } else {
      h.style.pointerEvents = "none";
    }
    h.setAttribute("aria-hidden", "true");
  };

  // Levels strictly below `root`, from the dialog's parent upward.
  let level: Element | null = dialog.parentElement;
  while (
    level !== null &&
    level !== root &&
    root.contains(level) &&
    level instanceof Element
  ) {
    for (const child of Array.from(level.children)) guard(child);
    level = level.parentElement;
  }
  // `root`'s own children once (covers dialogs mounted directly under root).
  for (const child of Array.from(root.children)) guard(child);

  return () => {
    for (const t of touched) {
      if (supportsInert) {
        t.el.inert = t.prevInert;
      } else {
        t.el.style.pointerEvents = t.prevPointerEvents;
      }
      if (t.prevAriaHidden === null) {
        t.el.removeAttribute("aria-hidden");
      } else {
        t.el.setAttribute("aria-hidden", t.prevAriaHidden);
      }
    }
    touched.length = 0;
  };
}
