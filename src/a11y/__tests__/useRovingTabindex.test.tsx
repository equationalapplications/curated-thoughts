import { render } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { useRovingTabindex } from "../useRovingTabindex";

function RovingFixture({
  active = true,
  disabled = false,
  orientation,
}: {
  active?: boolean;
  disabled?: boolean;
  orientation?: "vertical" | "horizontal" | "both";
}) {
  const ref = useRovingTabindex<HTMLUListElement>({ active, orientation });
  return (
    <ul ref={ref} data-testid="roving">
      <li>
        <button data-roving-item>Alpha</button>
      </li>
      <li>
        <button data-roving-item aria-disabled={disabled || undefined}>
          Beta
        </button>
      </li>
      <li>
        <button data-roving-item>Gamma</button>
      </li>
    </ul>
  );
}

function items(): HTMLElement[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>('[data-testid="roving"] [data-roving-item]'),
  );
}

function byText(text: string): HTMLElement {
  const el = items().find((i) => i.textContent === text);
  if (!el) throw new Error(`no roving item ${text}`);
  return el;
}

function expectRoving(expected: string) {
  for (const el of items()) {
    const want = el.textContent === expected ? "0" : "-1";
    expect(el.tabIndex, `${el.textContent} tabIndex`).toBe(Number(want));
  }
  expect(document.activeElement).toHaveTextContent(expected);
}

describe("useRovingTabindex", () => {
  it("marks the first item as the tab stop initially", () => {
    render(<RovingFixture />);
    expectRoving("Alpha");
    expect(byText("Alpha").tabIndex).toBe(0);
  });

  it("ArrowDown moves focus and the 0/-1 tabIndex pattern", async () => {
    render(<RovingFixture />);
    byText("Alpha").focus();
    await userEvent.keyboard("{ArrowDown}");
    expectRoving("Beta");
  });

  it("ArrowDown wraps from last to first", async () => {
    render(<RovingFixture />);
    byText("Gamma").focus();
    await userEvent.keyboard("{ArrowDown}");
    expectRoving("Alpha");
  });

  it("ArrowUp moves focus backward", async () => {
    render(<RovingFixture />);
    byText("Gamma").focus();
    await userEvent.keyboard("{ArrowUp}");
    expectRoving("Beta");
  });

  it("Home jumps to first, End jumps to last", async () => {
    render(<RovingFixture />);
    byText("Alpha").focus();
    await userEvent.keyboard("{End}");
    expectRoving("Gamma");
    await userEvent.keyboard("{Home}");
    expectRoving("Alpha");
  });

  it("skips aria-disabled items", async () => {
    render(<RovingFixture disabled />);
    byText("Alpha").focus();
    await userEvent.keyboard("{ArrowDown}");
    expectRoving("Gamma");
  });

  it("ignores horizontal arrows in vertical orientation", async () => {
    render(<RovingFixture />);
    byText("Alpha").focus();
    await userEvent.keyboard("{ArrowRight}");
    expectRoving("Alpha");
  });

  it("horizontal orientation responds to ArrowRight/ArrowLeft", async () => {
    render(<RovingFixture orientation="horizontal" />);
    byText("Alpha").focus();
    await userEvent.keyboard("{ArrowRight}");
    expectRoving("Beta");
    await userEvent.keyboard("{ArrowLeft}");
    expectRoving("Alpha");
  });

  it("does not manage items while inactive", () => {
    render(<RovingFixture active={false} />);
    expect(byText("Alpha").tabIndex).toBe(0);
  });
});
