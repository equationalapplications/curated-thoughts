import { useEffect, useRef } from "react";

const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

export interface FocusTrapOptions {
  active: boolean;
  /** Return true to let the browser handle this Tab (e.g. rich-text editors keep cursor control). */
  yieldTo?: (target: Element) => boolean;
  onEscape?: () => void;
}

export function useFocusTrap<T extends HTMLElement>(options: FocusTrapOptions) {
  const containerRef = useRef<T | null>(null);
  const { active, yieldTo, onEscape } = options;
  const previousFocus = useRef<Element | null>(null);

  useEffect(() => {
    if (!active || !containerRef.current) return;
    const container = containerRef.current;
    previousFocus.current = document.activeElement;

    const first = container.querySelector<HTMLElement>(FOCUSABLE);
    (first ?? container).focus();

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && onEscape) {
        event.stopPropagation();
        onEscape();
        return;
      }
      if (event.key !== "Tab") return;
      // yieldTo: ProseMirror/BlockNote keeps Tab + cursor control (spec §useFocusTrap).
      if (event.target instanceof Element && yieldTo?.(event.target)) return;

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
  }, [active, yieldTo, onEscape]);

  return containerRef;
}
