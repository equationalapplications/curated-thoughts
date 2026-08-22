import { useMemo } from "react";
import type { AppMode } from "../components/shell/ModeRail";
import type { NavTarget } from "./navigation";

export type CommandScope = "global" | `mode:${AppMode}`;

export interface Command {
  id: string;
  label: string;
  scope: CommandScope;
  /**
   * Palette-internal commands: canonical ids kept in the registry, but
   * never surfaced in search results — the palette's own key handling
   * implements their behavior.
   */
  internal?: boolean;
  run: () => void;
}

export interface CommandContext {
  navigate: (target: NavTarget) => void;
}

let context: CommandContext | null = null;

/**
 * AppShell registers the live navigation context on mount. Command `run`
 * closures read it lazily so COMMAND_REGISTRY can stay a module-level
 * constant even though `navigate` only exists inside the mounted hook.
 * Returns an unregister function for effect cleanup.
 */
export function registerCommandContext(ctx: CommandContext): () => void {
  context = ctx;
  return () => {
    if (context === ctx) context = null;
  };
}

/** Late-bound navigate for dynamically built palette entries. */
export function commandNavigate(target: NavTarget): void {
  context?.navigate(target);
}

function navigateTo(mode: AppMode): void {
  commandNavigate({ mode });
}

/**
 * The static command registry. "Static" = resolved at compile time — a
 * module-level constant, not a fetched service. Mode-scoped commands can
 * be contributed per-component through `useCommands(scope, extras)`.
 */
export const COMMAND_REGISTRY: Command[] = [
  { id: "nav.brain", label: "Go to Brain", scope: "global", run: () => navigateTo("brain") },
  { id: "nav.review", label: "Go to Review", scope: "global", run: () => navigateTo("review") },
  { id: "nav.library", label: "Go to Library", scope: "global", run: () => navigateTo("library") },
  { id: "nav.timeline", label: "Go to Timeline", scope: "global", run: () => navigateTo("timeline") },
  { id: "nav.tasks", label: "Go to Tasks", scope: "global", run: () => navigateTo("tasks") },
  { id: "nav.settings", label: "Go to Settings", scope: "global", run: () => navigateTo("settings") },
  { id: "palette.close", label: "Close the palette", scope: "global", internal: true, run: () => {} },
  { id: "palette.next", label: "Select the next result", scope: "global", internal: true, run: () => {} },
  { id: "palette.previous", label: "Select the previous result", scope: "global", internal: true, run: () => {} },
];

/**
 * Commands available for `scope`: every global registry command plus
 * registry commands scoped to that mode, merged with per-component
 * `extras` (on a duplicate id the extra wins). Extras are held to the
 * same scope rule as registry entries, so a mode-scoped extra surfaces
 * only in its own mode. Palette-internal commands are excluded — they
 * are dispatched by the palette's key handling, not chosen from the list.
 */
/** Stable default so omitting `extras` doesn't bust the useMemo each render. */
const NO_EXTRAS: Command[] = [];

export function useCommands(scope: CommandScope, extras: Command[] = NO_EXTRAS): Command[] {
  return useMemo(() => {
    const inScope = (c: Command) => c.scope === "global" || c.scope === scope;
    const fromRegistry = COMMAND_REGISTRY.filter((c) => !c.internal && inScope(c));
    const byId = new Map<string, Command>();
    for (const cmd of [...fromRegistry, ...extras.filter(inScope)]) byId.set(cmd.id, cmd);
    return [...byId.values()];
  }, [scope, extras]);
}
