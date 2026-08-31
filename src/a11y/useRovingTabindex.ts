import { useEffect, useRef } from "react";

export interface RovingOptions {
  active: boolean;
  itemSelector?: string; // default "[data-roving-item]"
  orientation?: "vertical" | "horizontal" | "both"; // default "vertical"
}

export function useRovingTabindex<T extends HTMLElement>(options: RovingOptions) {
  const containerRef = useRef<T | null>(null);
  const { active, itemSelector = "[data-roving-item]", orientation = "vertical" } = options;
  const currentRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const items = () =>
      Array.from(container.querySelectorAll<HTMLElement>(itemSelector)).filter(
        (el) => el.getAttribute("aria-disabled") !== "true",
      );

    const setTabStop = (el: HTMLElement) => {
      currentRef.current = el;
      for (const item of container.querySelectorAll<HTMLElement>(itemSelector)) {
        item.tabIndex = item === el ? 0 : -1;
      }
    };

    const initial = items()[0];
    if (!active || !initial) return;

    setTabStop(initial);

    const onFocusIn = (event: FocusEvent) => {
      const target = event.target;
      if (target instanceof HTMLElement && target.matches(itemSelector)) {
        currentRef.current = target;
      }
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (!["ArrowDown", "ArrowUp", "ArrowRight", "ArrowLeft", "Home", "End"].includes(event.key)) {
        return;
      }
      const enabled = items();
      if (enabled.length === 0) return;
      event.preventDefault();
      const current = currentRef.current ?? enabled[0];
      const index = enabled.indexOf(current);
      const vertical = orientation !== "horizontal";
      const horizontal = orientation !== "vertical";
      let next = -1;
      switch (event.key) {
        case "ArrowDown":
          if (vertical) next = (index + 1) % enabled.length;
          break;
        case "ArrowUp":
          if (vertical) next = (index - 1 + enabled.length) % enabled.length;
          break;
        case "ArrowRight":
          if (horizontal) next = (index + 1) % enabled.length;
          break;
        case "ArrowLeft":
          if (horizontal) next = (index - 1 + enabled.length) % enabled.length;
          break;
        case "Home":
          next = 0;
          break;
        case "End":
          next = enabled.length - 1;
          break;
      }
      if (next === -1) return;
      const target = enabled[next];
      setTabStop(target);
      target.focus();
    };

    container.addEventListener("keydown", onKeyDown, true);
    container.addEventListener("focusin", onFocusIn, true);
    return () => {
      container.removeEventListener("keydown", onKeyDown, true);
      container.removeEventListener("focusin", onFocusIn, true);
    };
  }, [active, itemSelector, orientation]);

  return containerRef;
}
