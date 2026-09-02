import { useRef } from "react";
import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useFocusTrap } from "../useFocusTrap";

type TrapVariant = "buttons" | "firstHidden";

function TrapFixture({
  active,
  onEscape,
  yieldTo,
  withContenteditable = false,
  variant = "buttons",
  children,
}: {
  active: boolean;
  onEscape?: () => void;
  yieldTo?: (target: Element) => boolean;
  withContenteditable?: boolean;
  /** "firstHidden": first button carries tabindex="-1" BEFORE activation. */
  variant?: TrapVariant;
  children?: React.ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useFocusTrap(ref, { active, onEscape, yieldTo });
  return (
    <div ref={ref} data-testid="trap">
      <button tabIndex={variant === "firstHidden" ? -1 : undefined}>First</button>
      <button>Middle</button>
      <button>Last</button>
      {withContenteditable && (
        <div contentEditable suppressContentEditableWarning data-testid="editor">
          {children}
        </div>
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

  // Boundary coverage for every valid editable-host state (CodeRabbit
  // round-3): "" and "plaintext-only" (and case variants of "true") are all
  // editable per HTML and must remain tab stops the trap holds.
  it.each(["", "TRUE", "plaintext-only"])(
    "keeps the trap closed when the last tab stop is contenteditable=%s",
    (state) => {
      mountWithPriorFocus(<TrapFixture active withContenteditable />);
      const editor = document.querySelector('[data-testid="editor"]') as HTMLElement;
      editor.setAttribute("contenteditable", state);
      const first = buttons()[0]!;
      first.focus();
      const event = new KeyboardEvent("keydown", {
        key: "Tab",
        shiftKey: true,
        bubbles: true,
        cancelable: true,
      });
      first.dispatchEvent(event);
      expect(document.activeElement).toBe(editor);
    },
  );

  // An explicit tabindex="-1" removes an element from SEQUENTIAL tab order
  // even when it matches a tag branch — the trap must not treat it as a
  // wrap-around candidate (CodeRabbit round-4; round-5 moved the tabindex=-1
  // onto the FIRST button BEFORE activation so the activate-state assertion
  // can only pass when the excluded node is genuinely skipped).
  it("excludes a tabindex=-1 button from sequential candidates on activate and Tab wrap", () => {
    mountWithPriorFocus(<TrapFixture active variant="firstHidden" />);
    const trap = document.querySelector('[data-testid="trap"]')!;
    // Activate-state: with the FIRST button excluded, activation must focus
    // the next sequential candidate (Middle), proving the -1 node is skipped.
    expect(document.activeElement).toHaveTextContent("Middle");
    // Wrap: from the last sequential candidate (Last), Tab must close the
    // cycle back to Middle, skipping the tabindex=-1 first node.
    buttons()[2]!.focus();
    buttons()[2]!.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true }),
    );
    expect(document.activeElement).toHaveTextContent("Middle");
    expect(trap.contains(document.activeElement)).toBe(true);
  });

  it("excludes a nested editable host carrying tabindex=-1 from sequential candidates", () => {
    mountWithPriorFocus(<TrapFixture active withContenteditable />);
    const editor = document.querySelector('[data-testid="editor"]') as HTMLElement;
    editor.setAttribute("tabindex", "-1");
    const last = buttons()[2]!;
    last.focus();
    // The editor is out of sequential order, so Tab from the last button must
    // wrap to the FIRST element, not land on the excluded editor.
    last.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true }),
    );
    expect(buttons()[0]).toBe(document.activeElement);
  });

  // Nested editable hosts WITHOUT a non-negative tabindex are out of
  // sequential tab order per HTML/MDN: only the outermost editing host is a
  // tab stop unless the inner one carries tabindex="0" (CodeRabbit round-6).
  it("excludes a nested editable host without tabindex from sequential candidates", () => {
    mountWithPriorFocus(
      <TrapFixture active withContenteditable>
        <div contentEditable suppressContentEditableWarning data-testid="nested-editor" />
      </TrapFixture>,
    );
    // With the nested host excluded, the TOP-LEVEL editor is the last
    // sequential candidate, so Tab from it must wrap to the FIRST button.
    // If the nested host is still counted, lastEl is the nested host and the
    // wrap never fires.
    const editor = document.querySelector('[data-testid="editor"]') as HTMLElement;
    editor.focus();
    editor.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true }),
    );
    expect(buttons()[0]).toBe(document.activeElement);
  });

  it("keeps a nested editable host with tabindex=0 in sequential candidates", () => {
    mountWithPriorFocus(
      <TrapFixture active withContenteditable>
        <div contentEditable suppressContentEditableWarning data-testid="nested-editor" />
      </TrapFixture>,
    );
    // tabindex="0" (set imperatively to match the fixture style above and
    // keep jsx-a11y/no-noninteractive-tabindex clean) opts the nested host
    // INTO sequential order as the LAST candidate, so Shift+Tab from the
    // first button must wrap to it — the trap drives the focus, so this
    // fails if the fix over-excludes.
    const nested = document.querySelector('[data-testid="nested-editor"]') as HTMLElement;
    nested.setAttribute("tabindex", "0");
    const first = buttons()[0]!;
    first.focus();
    first.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true, shiftKey: true }),
    );
    expect(nested).toBe(document.activeElement);
  });
});
