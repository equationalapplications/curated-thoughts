import { renderHook } from "@testing-library/react";
import {
  COMMAND_REGISTRY,
  commandNavigate,
  registerCommandContext,
  useCommands,
  type Command,
} from "../lib/commands";

describe("COMMAND_REGISTRY", () => {
  it("contains the built-in navigation and palette-internal commands", () => {
    const ids = COMMAND_REGISTRY.map((c) => c.id);
    expect(ids).toEqual(
      expect.arrayContaining([
        "nav.brain",
        "nav.review",
        "nav.library",
        "nav.timeline",
        "nav.tasks",
        "nav.settings",
        "palette.close",
        "palette.next",
        "palette.previous",
      ]),
    );
    for (const cmd of COMMAND_REGISTRY.filter((c) => c.id.startsWith("palette."))) {
      expect(cmd.internal).toBe(true);
    }
  });

  it("nav commands dispatch through the registered context", () => {
    const navigate = vi.fn();
    const unregister = registerCommandContext({ navigate });
    COMMAND_REGISTRY.find((c) => c.id === "nav.library")!.run();
    expect(navigate).toHaveBeenCalledWith({ mode: "library" });
    unregister();
  });

  it("nav commands are inert (no throw) once the context is unregistered", () => {
    const navigate = vi.fn();
    const unregister = registerCommandContext({ navigate });
    unregister();
    COMMAND_REGISTRY.find((c) => c.id === "nav.brain")!.run();
    expect(navigate).not.toHaveBeenCalled();
  });

  it("commandNavigate reaches the registered context", () => {
    const navigate = vi.fn();
    const unregister = registerCommandContext({ navigate });
    commandNavigate({ mode: "brain", entityId: "ent_1" });
    expect(navigate).toHaveBeenCalledWith({ mode: "brain", entityId: "ent_1" });
    unregister();
  });
});

describe("useCommands", () => {
  it("returns global commands for any scope and hides palette internals", () => {
    const { result } = renderHook(() => useCommands("mode:brain"));
    const ids = result.current.map((c) => c.id);
    expect(ids).toContain("nav.brain");
    expect(ids).toContain("nav.settings");
    expect(ids.some((id) => id.startsWith("palette."))).toBe(false);
  });

  it("mode-scoped extras appear only in their own mode scope", () => {
    const extra: Command = {
      id: "brain.custom",
      label: "Custom brain action",
      scope: "mode:brain",
      run: vi.fn(),
    };
    const inBrain = renderHook(() => useCommands("mode:brain", [extra]));
    expect(inBrain.result.current.map((c) => c.id)).toContain("brain.custom");
    const inReview = renderHook(() => useCommands("mode:review", [extra]));
    expect(inReview.result.current.map((c) => c.id)).not.toContain("brain.custom");
  });

  it("extras override registry entries with the same id", () => {
    const override: Command = {
      id: "nav.brain",
      label: "My Brain",
      scope: "global",
      run: vi.fn(),
    };
    const { result } = renderHook(() => useCommands("global", [override]));
    const brain = result.current.find((c) => c.id === "nav.brain")!;
    expect(brain.label).toBe("My Brain");
    expect(result.current.filter((c) => c.id === "nav.brain")).toHaveLength(1);
  });
});
