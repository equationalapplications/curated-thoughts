import { render, screen, fireEvent } from "@testing-library/react";
import { describe, test, expect, vi } from "vitest";
import { parseWikilinks, WikilinkText } from "../components/brain/WikilinkText";

describe("parseWikilinks", () => {
  test("splits text and link segments", () => {
    expect(parseWikilinks("Works with [[Alpha]] and [[Beta Team]].")).toEqual([
      { type: "text", value: "Works with " },
      { type: "link", value: "Alpha" },
      { type: "text", value: " and " },
      { type: "link", value: "Beta Team" },
      { type: "text", value: "." },
    ]);
  });

  test("handles text with no links", () => {
    expect(parseWikilinks("No links here")).toEqual([
      { type: "text", value: "No links here" },
    ]);
  });

  test("handles unclosed wikilinks as text", () => {
    expect(parseWikilinks("Unclosed [[Alpha")).toEqual([
      { type: "text", value: "Unclosed [[Alpha" },
    ]);
  });
});

describe("WikilinkText component", () => {
  test("clicking a chip fires onNavigate with the entity name", () => {
    const onNavigate = vi.fn();
    render(<WikilinkText text="See [[Project X]] for details." onNavigate={onNavigate} />);
    fireEvent.click(screen.getByRole("button", { name: "Project X" }));
    expect(onNavigate).toHaveBeenCalledWith("Project X");
  });

  test("renders multiple wikilinks as separate chips", () => {
    const onNavigate = vi.fn();
    render(<WikilinkText text="[[First]] and [[Second]]" onNavigate={onNavigate} />);
    const buttons = screen.getAllByRole("button");
    expect(buttons).toHaveLength(2);
    expect(buttons[0]).toHaveTextContent("First");
    expect(buttons[1]).toHaveTextContent("Second");
  });

  test("renders plain text without any buttons", () => {
    const onNavigate = vi.fn();
    render(<WikilinkText text="Just plain text" onNavigate={onNavigate} />);
    const buttons = screen.queryAllByRole("button");
    expect(buttons).toHaveLength(0);
  });
});
