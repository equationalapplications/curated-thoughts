import { useEffect } from "react";

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  if (target.isContentEditable) return true;
  return false;
}

interface Options {
  enabled: boolean;
  onApprove: () => void;
  onReject: () => void;
  onNext: () => void;
  onPrev: () => void;
  onFocusEditor: () => void;
}

export function useReviewKeyboard({
  enabled,
  onApprove,
  onReject,
  onNext,
  onPrev,
  onFocusEditor,
}: Options): void {
  useEffect(() => {
    if (!enabled) return;

    function handleKeyDown(event: KeyboardEvent) {
      if (isEditableTarget(event.target)) return;
      if (event.metaKey || event.ctrlKey || event.altKey) return;

      switch (event.key) {
        case "a":
          event.preventDefault();
          onApprove();
          break;
        case "r":
          event.preventDefault();
          onReject();
          break;
        case "e":
          event.preventDefault();
          onFocusEditor();
          break;
        case " ":
        case "j":
          event.preventDefault();
          onNext();
          break;
        case "k":
          event.preventDefault();
          onPrev();
          break;
        default:
          break;
      }
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [enabled, onApprove, onReject, onNext, onPrev, onFocusEditor]);
}
