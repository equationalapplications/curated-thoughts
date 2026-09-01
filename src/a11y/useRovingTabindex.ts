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
    const allItems = () =>
      Array.from(container.querySelectorAll<HTMLElement>(itemSelector));

    const setTabStop = (el: HTMLElement) => {
      currentRef.current = el;
      for (const item of allItems()) {
        item.tabIndex = item === el ? 0 : -1;
      }
    };

    if (!active) {
      // Deactivate: hand tab stops back to the browser (all default) so the
      // group stays reachable by Tab, and drop any stale currentRef.
      currentRef.current = null;
      for (const item of allItems()) item.tabIndex = 0;
      return;
    }

    // Reconcile whenever the item set changes: setTabStop writes 0/-1 across
    // ALL items, so newly added items can never leave two tabIndex=0 elements
    // behind, and a removed/disabled currentRef is replaced by a contained,
    // enabled item. childList only — attribute flips (aria-disabled) are
    // handled lazily by the items() filter on the next keypress.
    const reconcile = () => {
      const enabled = items();
      if (enabled.length === 0) {
        currentRef.current = null;
        return;
      }
      const current = currentRef.current;
      if (current && current.isConnected && enabled.includes(current)) {
        setTabStop(current);
      } else {
        setTabStop(enabled[0]);
      }
    };

    reconcile();
    const observer = new MutationObserver(reconcile);
    observer.observe(container, { childList: true, subtree: true });

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
      const current = currentRef.current ?? enabled[0];
      const index = enabled.indexOf(current);
      // Focus outside the enabled set (e.g. a disabled item) starts from 0.
      const safeIndex = index === -1 ? 0 : index;
      const vertical = orientation !== "horizontal";
      const horizontal = orientation !== "vertical";
      let next = -1;
      switch (event.key) {
        case "ArrowDown":
          if (vertical) next = (safeIndex + 1) % enabled.length;
          break;
        case "ArrowUp":
          if (vertical) next = (safeIndex - 1 + enabled.length) % enabled.length;
          break;
        case "ArrowRight":
          if (horizontal) next = (safeIndex + 1) % enabled.length;
          break;
        case "ArrowLeft":
          if (horizontal) next = (safeIndex - 1 + enabled.length) % enabled.length;
          break;
        case "Home":
          next = 0;
          break;
        case "End":
          next = enabled.length - 1;
          break;
      }
      // preventDefault only when a navigation actually happens: keys this
      // orientation ignores must keep native scroll behavior (e.g. vertical
      // list must not swallow ArrowLeft/ArrowRight page scroll).
      if (next === -1) return;
      event.preventDefault();
      const target = enabled[next];
      setTabStop(target);
      target.focus();
    };

    container.addEventListener("keydown", onKeyDown, true);
    container.addEventListener("focusin", onFocusIn, true);
    return () => {
      container.removeEventListener("keydown", onKeyDown, true);
      container.removeEventListener("focusin", onFocusIn, true);
      observer.disconnect();
      // Cleanup restores default tabbability so the group stays reachable by
      // Tab after deactivation, and a stale currentRef can't leak into the
      // next activation.
      currentRef.current = null;
      for (const item of allItems()) item.tabIndex = 0;
    };
  }, [active, itemSelector, orientation]);

  return containerRef;
}
