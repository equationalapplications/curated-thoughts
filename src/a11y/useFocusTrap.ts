import { useEffect, useRef, type RefObject } from "react";

const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
  // All valid editable-host states (CodeRabbit round-3): contenteditable=""
  // and "true"/"plaintext-only" (case-insensitive) are all editable per HTML.
  '[contenteditable=""]',
  '[contenteditable="true" i]',
  '[contenteditable="plaintext-only" i]',
].join(",");

/**
 * An element is an editing host when its contenteditable attribute is one of
 * the valid enabling states ("" / "true" / "plaintext-only", case-insensitive).
 */
function isEditableHost(el: Element): boolean {
  const ce = el.getAttribute("contenteditable");
  if (ce === null) return false;
  const v = ce.toLowerCase();
  return ce === "" || v === "true" || v === "plaintext-only";
}

/**
 * Sequential-tab candidates for wrap-around. Mirrors the CSS list but adds
 * the rules CSS can't express across branches:
 *  - an explicit tabindex="-1" is OUT of sequential tab order even when the
 *    element matches a tag branch (e.g. <button tabindex="-1">, or an
 *    editable host carrying tabindex="-1"). (CodeRabbit round-4.)
 *  - an editable host WITHOUT a tabindex is out of sequential tab order when
 *    it is nested inside another editing host: only the outermost editing
 *    host is a tab stop unless the inner one carries a non-negative tabindex
 *    (HTML sequential navigation; CodeRabbit round-6).
 */
function isTabCandidate(el: HTMLElement, container: ParentNode): boolean {
  const tabindex = el.getAttribute("tabindex");
  if (tabindex === "-1") return false;
  if (tabindex !== null) return true; // explicit non-negative tabindex opts in
  if (!isEditableHost(el)) return true;
  let ancestor = el.parentElement;
  while (ancestor && ancestor !== container) {
    if (isEditableHost(ancestor)) return false;
    ancestor = ancestor.parentElement;
  }
  return true;
}

function tabCandidates(root: ParentNode): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter((el) =>
    isTabCandidate(el, root),
  );
}

export interface FocusTrapOptions {
  active: boolean;
  /** Return true to let the browser handle this Tab (e.g. rich-text editors keep cursor control). */
  yieldTo?: (target: Element) => boolean;
  onEscape?: () => void;
}

/**
 * Ref-first API per spec §3 (`useFocusTrap(ref, { active, yieldTo })`): the
 * caller owns the ref so a sibling effect that must release BEFORE the trap
 * restores focus (e.g. the inert guard) can be declared first — React runs
 * unmount cleanups in declaration order. With a hook-created ref, the trap
 * is always registered first and the release/restore order is unfixable.
 */
export function useFocusTrap<T extends HTMLElement>(
  containerRef: RefObject<T | null>,
  options: FocusTrapOptions,
): void {
  const { active } = options;
  // Callbacks live in refs so consumer re-renders don't tear down and
  // re-register listeners — which would re-focus the first element and steal
  // focus mid-interaction. The effect depends only on `active`.
  const yieldToRef = useRef(options.yieldTo);
  const onEscapeRef = useRef(options.onEscape);
  useEffect(() => {
    yieldToRef.current = options.yieldTo;
    onEscapeRef.current = options.onEscape;
  });
  const previousFocus = useRef<Element | null>(null);

  useEffect(() => {
    if (!active || !containerRef.current) return;
    const container = containerRef.current;
    previousFocus.current = document.activeElement;

    const first = tabCandidates(container)[0];
    (first ?? container).focus();

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && onEscapeRef.current) {
        event.stopPropagation();
        onEscapeRef.current();
        return;
      }
      if (event.key !== "Tab") return;
      // yieldTo: ProseMirror/BlockNote keeps Tab + cursor control (spec §useFocusTrap).
      if (event.target instanceof Element && yieldToRef.current?.(event.target)) return;

      const focusables = tabCandidates(container);
      if (focusables.length === 0) return;
      const firstEl = focusables[0];
      const lastEl = focusables[focusables.length - 1];
      const current = document.activeElement;
      const inside = current instanceof Node && container.contains(current);
      if (event.shiftKey) {
        if (current === firstEl || !inside) {
          event.preventDefault();
          lastEl.focus();
        }
      } else if (current === lastEl || !inside) {
        event.preventDefault();
        firstEl.focus();
      }
    };

    container.addEventListener("keydown", onKeyDown, true);
    return () => {
      container.removeEventListener("keydown", onKeyDown, true);
      const prev = previousFocus.current;
      if (prev instanceof HTMLElement) prev.focus();
    };
  }, [active, containerRef]);
}
