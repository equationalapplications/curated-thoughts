# Tauri listener registration: no unhandled rejections (issue #236)

**Date:** 2026-09-28
**Status:** Draft
**Branch:** fix/issue-236-listen-registration
**Priority:** Medium (bug, low severity — console noise + silently missing listeners)

## Problem

PR #235 hardened the *unlisten* half of every frontend Tauri subscription
(`safeUnlisten`), but the *listen* half has no handler: a rejected
`listen()`/`onDragDropEvent()` produces an `unhandledrejection` and the
listener silently never exists. Worst sites: `useProviderHealth.ts:44-83`
(`Promise.all(...).then` with no `.catch` — immediate unhandled rejection AND
leaked already-subscribed listeners), the fire-and-forget `setup()` fns
(GenerationPanel, StepWatchItThink, StepFastembed — the last also skips
`initFastembed()` on rejection, leaving the panel stuck on "loading"), and
`AppShell.tsx:148-177` (the flagged `.then` chain, never handled).

Full investigation (complete call-site inventory — 20+ sites across three
patterns plus the `tauri.ts` wrapper — failure shapes, Opus verdict history):
`2026-09-28-issue236-listen-registration-investigation.md` (same directory).

## Approach

1. **`guardListen(subscription, context)` helper** in `src/lib/events.ts`
   next to `safeUnlisten`: attaches a logging `.catch` at subscription
   creation (`[events] listen failed (<context>)` — exactly one warn), returns
   the ORIGINAL promise (same identity; existing cleanup keeps working;
   awaiting callers still see the rejection).
2. **Wrap every subscription creation** in the inventory with
   `guardListen(promise, "<event-name>")`.
3. **Promise.all sites (four: useProviderHealth, GenerationPanel,
   StepWatchItThink, StepFastembed):** subscriptions set up with
   `Promise.allSettled` (fail-fast `Promise.all` both leaks listeners and
   skips remaining setup); cleanup holds the guarded PROMISE array and
   iterates `promises.forEach((p) => void safeUnlisten(p))` — never a bare
   array (safeUnlisten takes a single promise; forEach is the existing local
   idiom). Rejections are logged ONCE by guardListen; no added `.catch`
   anywhere (double-log guard).
4. **StepFastembed:** `initFastembed()` runs after allSettled regardless of
   subscription outcomes (degrade progress UI; never stuck on "loading").
5. **StepModel (own rule):** keeps ref-array cleanup with
   `await Promise.allSettled(unlistens.current)` — a progress-listener
   rejection must NOT abort the in-flight auto-install.
6. **ModelPanel / StepOllama:** `await` moved inside `try`, unlisten variable
   declared before `try`, `safeUnlisten` in `finally`; on rejection the pull
   proceeds without progress display (aborting an in-flight pull would be
   worse).
7. **AppShell drag-drop** converted to the standard pattern:
   `const promise = guardListen(getCurrentWindow().onDragDropEvent(...),
   "drag-drop")` + cleanup `void safeUnlisten(promise)` (the `cancelled` flag
   dance becomes unnecessary).
8. **useWikiStatus / tauri.ts `onWikiStatusChange`:** normalize to
   guardListen for consistent `[events]` formatting; KEEP existing `.catch`
   semantics (a bare `.then` chain on a guarded promise re-creates the
   unhandled rejection; guard+catch on one chain double-logs).

**Rejected alternatives:** guarding inside the `on*` wrappers (double-logs at
sites with their own handling; changes return contracts mid-file);
`Promise.allSettled`-only without held-promise cleanup (still leaks);
fail-fast with `.catch` on `setup()` (skips remaining setup, still
unhandled at the invocation).

## Error handling

The one failure mode (subscription rejection) maps to exactly one
`console.warn` per failed subscription; no behavior change otherwise. All
call sites keep functioning with the listener absent (that is today's
de-facto behavior, just without the unhandled rejection).

## Testing

- `guardListen`: rejects → logs once with context, returns same identity
  (`toBe`), extends `src/__tests__/safeUnlisten.test.ts` (PR #235 pattern).
- AppShell component test: `onDragDropEvent` rejecting — use a SHARED mock
  with `mockReturnValueOnce` (`test-setup.ts:197-202` creates a fresh vi.fn
  per `getCurrentWindow()` call); one warn, no unhandledrejection (vitest
  fails suites on unhandled rejections).
- useProviderHealth: one of six subscriptions rejects → the other five
  unlisten fns still fire on unmount (leak rule).
- StepFastembed: subscription rejects AND `initFastembed()` still runs.
- Inventory cross-check (post-conversion gate): the explicit-creator rg from
  the investigation (with `(<[^>]*>)?\(` for generic-parameterized calls;
  known residual gap: multi-line generics — diff against the doc's inventory
  too).

## Out of scope / open questions

- Rust-side listener health: out of scope (registration failure is
  host-level; the app cannot do more than log).
- Severity deliberately unchanged (low): no user-visible regression today,
  this is hygiene completing the PR #235 contract.

Fixes #236.
