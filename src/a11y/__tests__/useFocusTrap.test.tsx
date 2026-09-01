import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useFocusTrap } from "../useFocusTrap";

function TrapFixture({
  active,
  onEscape,
  yieldTo,
  withContenteditable = false,
}: {
  active: boolean;
  onEscape?: () => void;
  yieldTo?: (target: Element) => boolean;
  withContenteditable?: boolean;
}) {
  const ref = useFocusTrap<HTMLDivElement>({ active, onEscape, yieldTo });
  return (
    <div ref={ref} data-testid="trap">
      <button>First</button>
      <button>Middle</button>
      <button>Last</button>
      {withContenteditable && (
        <div contentEditable suppressContentEditableWarning data-testid="editor" />
      )}
    </div>
  );
}

/** Focuses a button outside React, mounts the trap, returns the prior node. */
function mountWithPriorFocus(ui: React.ReactElement) {
  const prior = document.createElement("button");
  prior.textContent = "Outside trigger";
  document.body.appendChild(prior);
  prior.focus();
  const utils = render(ui);
  return { prior, ...utils };
}

function buttons(): HTMLElement[] {
  return Array.from(document.querySelectorAll<HTMLElement>('[data-testid="trap"] > button'));
}

function tabEvent(target: Element) {
  const event = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
  const preventDefault = vi.fn();
  Object.defineProperty(event, "preventDefault", { value: preventDefault });
  target.dispatchEvent(event);
  return preventDefault;
}


describe("useFocusTrap", () => {
  it("focuses the first focusable element on activate", () => {
    mountWithPriorFocus(<TrapFixture active />);
    expect(document.activeElement).toHaveTextContent("First");
  });

  it("cycles Tab from last back to first", () => {
    mountWithPriorFocus(<TrapFixture active />);
    const last = buttons()[2]!;
    last.focus();
    last.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true }),
    );
    expect(document.activeElement).toHaveTextContent("First");
  });

  it("cycles Shift+Tab from first back to last", () => {
    mountWithPriorFocus(<TrapFixture active />);
    expect(document.activeElement).toHaveTextContent("First");
    const event = new KeyboardEvent("keydown", {
      key: "Tab",
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    });
    (document.activeElement as HTMLElement).dispatchEvent(event);
    expect(document.activeElement).toHaveTextContent("Last");
  });

  it("restores focus to the previously focused element on deactivate", () => {
    const { prior, rerender } = mountWithPriorFocus(<TrapFixture active />);
    expect(document.activeElement).toHaveTextContent("First");
    rerender(<TrapFixture active={false} />);
    expect(document.activeElement).toBe(prior);
  });

  it("fires onEscape and stops propagation", () => {
    const onEscape = vi.fn();
    const stopPropagation = vi.fn();
    render(<TrapFixture active onEscape={onEscape} />);
    const trap = document.querySelector('[data-testid="trap"]')!;
    const event = new KeyboardEvent("keydown", { key: "Escape", bubbles: true });
    Object.defineProperty(event, "stopPropagation", { value: stopPropagation });
    trap.dispatchEvent(event);
    expect(onEscape).toHaveBeenCalledTimes(1);
    expect(stopPropagation).toHaveBeenCalledTimes(1);
  });

  it("does NOT intercept Tab when yieldTo matches the event target", () => {
    mountWithPriorFocus(
      <TrapFixture
        active
        withContenteditable
        yieldTo={(t) => t.getAttribute("contenteditable") === "true"}
      />,
    );
    const editor = document.querySelector('[data-testid="editor"]') as HTMLElement;
    editor.focus();
    const preventDefault = tabEvent(editor);
    expect(preventDefault).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(editor);
  });

  it("intercepts Tab when yieldTo does not match (Shift+Tab wraps to the container's last tab stop)", () => {
    mountWithPriorFocus(
      <TrapFixture
        active
        withContenteditable
        yieldTo={(t) => t.getAttribute("contenteditable") === "true"}
      />,
    );
    const first = buttons()[0]!;
    const editor = document.querySelector('[data-testid="editor"]') as HTMLElement;
    first.focus();
    const event = new KeyboardEvent("keydown", {
      key: "Tab",
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    });
    first.dispatchEvent(event);
    // The contenteditable editor is a real tab stop (CodeRabbit follow-up),
    // so it is the container's last focusable and receives the Shift+Tab wrap.
    expect(document.activeElement).toBe(editor);
  });

  it("cycles forward from a [contenteditable=true] last stop back to the first element", () => {
    mountWithPriorFocus(<TrapFixture active withContenteditable />);
    const first = buttons()[0]!;
    const editor = document.querySelector('[data-testid="editor"]') as HTMLElement;
    editor.focus();
    // The editor is the container's last tab stop; native Tab from the last
    // button reaches it without interception (jsdom can't simulate that), and
    // the trap must close the cycle from here back to the first element.
    const preventDefault = tabEvent(editor);
    expect(preventDefault).toHaveBeenCalled();
    expect(document.activeElement).toBe(first);
  });
});
