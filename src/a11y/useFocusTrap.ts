import { useEffect, useRef } from "react";

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

export interface FocusTrapOptions {
  active: boolean;
  /** Return true to let the browser handle this Tab (e.g. rich-text editors keep cursor control). */
  yieldTo?: (target: Element) => boolean;
  onEscape?: () => void;
}

export function useFocusTrap<T extends HTMLElement>(options: FocusTrapOptions) {
  const containerRef = useRef<T | null>(null);
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

    const first = container.querySelector<HTMLElement>(FOCUSABLE);
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

      const focusables = Array.from(
        container.querySelectorAll<HTMLElement>(FOCUSABLE),
      );
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
  }, [active]);

  return containerRef;
}
