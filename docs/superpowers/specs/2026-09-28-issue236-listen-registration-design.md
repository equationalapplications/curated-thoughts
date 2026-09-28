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
5. **StepModel (own rule, wording direction fixed per spec m1):** BEFORE the
   auto-install starts, keep ref-array cleanup with
   `await Promise.allSettled(unlistens.current)` — a progress-listener
   rejection must NOT abort the in-flight auto-install. (allSettled is only
   for the cleanup await — no allSettled at single-listener sites where the
   existing try/finally already handles ordering; spec m2.)
6. **ModelPanel / StepOllama (ordering corrected, Opus spec M1):** the
   `await onPullProgress(...)` fires BEFORE `pullModel()` — a listener
   rejection must not abort a pull that hasn't started. Rule: on rejection,
   log-and-continue — guardListen has ALREADY logged once, so the surrounding
   code catches ONLY to prevent the escape and MUST NOT log again
   (`.catch(() => {})` shape; this is the one sanctioned silent catch, scoped
   to sites where guardListen owns the log — R1). On rejection set progress
   display to "unavailable" and STILL call `pullModel()`; `unlisten`
   declared before `try`, `safeUnlisten` in `finally`.
7. **setupWiki (Opus spec M3):** the four `await listen(...)` calls
   (`wiki.ts:450-464`) are boot-gating — convert to `Promise.allSettled`
   over guarded listens so one rejected subscription never fails boot and
   never double-logs; features relying on a rejected listener degrade
   (auto-heal trigger, classifier-refresh trigger) while the rest of the
   wiki engine comes up. **Teardown (Opus c2 R3):** the cleanup iterates the
   SETTLED results and calls safeUnlisten only on fulfilled entries —
   never invoke an `undefined` unlisten from a rejected one; the
   `startAutoHeal`/`startAutoMaintenance` subscription sites get the same
   settled-cleanup treatment.
8. **useWikiStatus / tauri.ts `subscribeEntityStatus`** (name corrected from
   the investigation's `onWikiStatusChange`): KEEP its existing
   `.catch(console.error)` and do NOT add guardListen there — guard+catch on
   the same chain is exactly the double-log this spec forbids (Opus spec
   M4; supersedes the earlier "normalize" wording). **Per CALL SITE, not per
   wrapper (Opus c2 R4):** the exemption applies to the useWikiStatus call
   site(s) that already handle rejection; any FUTURE caller of
   `subscribeEntityStatus` without its own `.catch` needs guardListen.

**Precedence rule (Opus c2 R2):** where this spec and the companion
investigation doc disagree (ModelPanel/StepOllama ordering, the wrapper name,
the rg creator list), THIS SPEC WINS — the investigation records the review
history, not the final ruling.

**Logging contract (Opus c2 R6, replaces the blanket "exactly one warn"):**
one `console.warn` per FAILED subscription, emitted by guardListen except at
exempted sites (useWikiStatus's own console.error). Successful subscriptions
are silent — today's behavior is unchanged for them.

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
- ModelPanel/StepOllama: listener rejection → pull STILL runs, progress shows
  unavailable, one warn (ordering-corrected rule); unlisten in `finally`.
- setupWiki: one rejected listen of four → boot completes, other three
  subscriptions live, exactly one warn.
- useWikiStatus/subscribeEntityStatus: unchanged behavior (existing catch
  remains the only handler — no guard added).
- Inventory cross-check (post-conversion gate; baseline corrected per Opus
  spec M2 — the c3 command missed `Progress/Done/Error` wrapper variants and
  named a nonexistent wrapper; the plan re-derives the creator list from
  `events.ts` exports + `subscribeEntityStatus` and reconciles ~34 sites):
  every hit guarded or covered by an explicit site rule (setupWiki
  allSettled, subscribeEntityStatus existing-catch, AppShell pattern).
  Ripgrep notes retained from the investigation: `--type ts` covers tsx;
  `(<[^>]*>)?\(` required for generic-parameterized calls; multi-line
  generics evade the pattern — diff against the site list too.

## Out of scope / open questions

- Rust-side listener health: out of scope (registration failure is
  host-level; the app cannot do more than log).
- Severity deliberately unchanged (low): no user-visible regression today,
  this is hygiene completing the PR #235 contract.

Fixes #236.
