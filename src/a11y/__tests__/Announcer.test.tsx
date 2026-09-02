import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AnnouncerProvider, useAnnouncer } from "../Announcer";

type Msg = { text: string; politeness?: "polite" | "assertive" };

function Probe({ messages }: { messages: Msg[] }) {
  const { announce } = useAnnouncer();
  return (
    <button onClick={() => messages.forEach((m) => announce(m.text, m.politeness))}>
      go
    </button>
  );
}

function announceAll(messages: Msg[]): HTMLElement {
  render(
    <AnnouncerProvider>
      <Probe messages={messages} />
    </AnnouncerProvider>,
  );
  const button = screen.getByRole("button", { name: "go" });
  fireEvent.click(button);
  return button;
}

function politeRegion(): HTMLElement {
  return document.querySelector('div[aria-live="polite"].a11y-announcer')!;
}

function assertiveRegion(): HTMLElement {
  return document.querySelector('div[role="alert"].a11y-announcer')!;
}

function regionTexts(): Array<string | null> {
  return Array.from(politeRegion().querySelectorAll("p")).map((p) => p.textContent);
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("AnnouncerProvider", () => {
  it("renders a polite announcement into the aria-live=polite region", () => {
    announceAll([{ text: "Note saved" }]);
    expect(politeRegion()).toHaveTextContent("Note saved");
    expect(politeRegion()).toHaveAttribute("aria-atomic", "true");
  });

  it("a lone announce() reaches the DOM immediately (no drain penalty)", () => {
    announceAll([{ text: "instant" }]);
    // Same tick, zero timer advancement: the write already happened.
    expect(regionTexts()).toEqual(["instant"]);
  });

  it("spec acceptance: two same-tick calls reach the DOM in order WITH the 150ms gap between them", () => {
    announceAll([{ text: "one" }, { text: "two" }]);
    // First writes immediately; the second must NOT be in the DOM yet.
    expect(regionTexts()).toEqual(["one"]);
    act(() => {
      vi.advanceTimersByTime(150);
    });
    expect(regionTexts()).toEqual(["two"]);
  });

  it("drains three same-tick messages sequentially, one per floor interval", () => {
    announceAll([{ text: "first" }, { text: "second" }, { text: "third" }]);
    expect(regionTexts()).toEqual(["first"]);
    act(() => {
      vi.advanceTimersByTime(150);
    });
    expect(regionTexts()).toEqual(["second"]);
    act(() => {
      vi.advanceTimersByTime(150);
    });
    expect(regionTexts()).toEqual(["third"]);
    act(() => {
      vi.advanceTimersByTime(150);
    });
    expect(regionTexts()).toEqual([]);
  });

  it("collapses an identical message announced together (same tick)", () => {
    announceAll([{ text: "Saved" }, { text: "Saved" }]);
    expect(politeRegion().querySelectorAll("p")).toHaveLength(1);
    act(() => {
      vi.advanceTimersByTime(150);
    });
    // The collapsed duplicate never got its own write.
    expect(regionTexts()).toEqual([]);
  });

  it("announces again after the floor elapses (no stale collapse window)", () => {
    const go = announceAll([{ text: "Saved" }]);
    // After the 150ms floor both the removal and the collapse window have closed,
    // so a repeat announcement must produce a fresh, separate entry.
    act(() => {
      vi.advanceTimersByTime(150);
    });
    expect(politeRegion().querySelectorAll("p")).toHaveLength(0);
    fireEvent.click(go);
    expect(politeRegion().querySelectorAll("p")).toHaveLength(1);
  });

  it("routes assertive messages into the role=alert region", () => {
    announceAll([{ text: "Connection lost", politeness: "assertive" }]);
    expect(assertiveRegion()).toHaveTextContent("Connection lost");
    expect(politeRegion()).not.toHaveTextContent("Connection lost");
  });

  it("removes a message after its 150ms floor", () => {
    announceAll([{ text: "transient" }]);
    expect(politeRegion()).toHaveTextContent("transient");
    act(() => {
      vi.advanceTimersByTime(150);
    });
    expect(politeRegion()).not.toHaveTextContent("transient");
  });

  it("throws when useAnnouncer is used without a provider", () => {
    function Bare() {
      useAnnouncer();
      return null;
    }
    expect(() => render(<Bare />)).toThrow(/useAnnouncer requires <AnnouncerProvider>/);
  });
});
