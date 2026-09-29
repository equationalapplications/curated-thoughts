# Investigation — issue #236: unhandled rejection on Tauri listener registration failure

**Date:** 2026-09-28
**Status:** Investigation (Step 0 of the delivery flow)
**Issue:** #236 (bug, javascript, low severity — found during `/code-review high` of PR #235)
**Evidence tags:** [V] = controller-verified; [C] = child-reported

## Repro of the failure class

Not reproducible against a live app window without a hostile/destroyed Tauri
host; the failure class is a rejected `listen()`/`onDragDropEvent()` promise.
Repro in-test: `mockRejectedValueOnce` on the `@tauri-apps/api/event` (and
`window`) mocks that `src/test-setup.ts` already provides globally, then mount /
invoke and assert exactly one `console.warn` and no `unhandledrejection`. That
is the acceptance test of the fix and the plan's RED test. [C] test-infra
shapes, [V] anchor lines below.

## Code-level current state [V]

Verified at repo `eab433f`. The issue's call-site inventory is **complete and
accurate**; the child found no additional sites.

**The only existing helper** — `safeUnlisten` (`src/lib/events.ts:1-40`):
handles *cleanup* failures (with a retry ladder, final `console.warn`), and
tolerates a rejected subscription promise (swallows on await). Nothing handles
*registration* failure.

**Failure shapes by call-site pattern (inventory per Opus cycle-1 re-verification):**

- **Pattern A** (hold promise, cleanup via `safeUnlisten`): rejection unhandled
  only while mounted. Sites: `usePrivacyMode.ts:83-91`, `useVaultFiles.ts:15-18`,
  `SplashScreen.tsx:31-39` (three raw promises), `AppShell.tsx:135`
  (`onVaultSwitched`), `AppShell.tsx:234` (`config-malformed`).
- **Pattern B** (await/Promise.all of subscriptions):
  - `useProviderHealth.ts:44-83` — `void Promise.all([...6 wrappers]).then(...)`
    **no `.catch`** → immediate unhandled rejection; worst site. Also: listeners
    that DID subscribe are leaked if one rejects (unlistens stored only after
    `Promise.all` resolves).
  - `GenerationPanel.tsx:33-53`, `StepWatchItThink.tsx:38-66` — fire-and-forget
    `setup()` async fns awaiting `Promise.all`, invoked bare (same leak shape).
  - `StepFastembed.tsx:18-48` — Pattern A+B hybrid: two pending promises AND an
    awaited `Promise.all` in `setup()`; on rejection `initFastembed()` never
    runs → panel stuck on "loading".
  - `StepModel.tsx:69-80` — pending promises in a ref; `await
    Promise.all(unlistens.current)` at :83 sits OUTSIDE its `try` (same escape
    shape as ModelPanel).
  - `wiki.ts:545-552` — `startAutoHeal` `vault-event` subscription;
    `wiki.ts:450-464` — `setupWiki` with four `await listen(...)`; `wiki.ts:590-598`
    — subscription list in `startAutoMaintenance`.
  - `ModelPanel.tsx:33-53` (`await onPullProgress` at :38, before its `try`),
    `StepOllama.tsx` (await at :32, ends :44) — rejection escapes the function.
- **Pattern C** — `AppShell.tsx:148-177` (controller-verified in full): the
  `.then()`-chain with no `.catch` the issue quotes. Rejection never handled.
- **Wrapper outside events.ts:** `tauri.ts:582` `onWikiStatusChange` — its own
  call sites need the same treatment.

**Working precedent [C, shape verified]:** `useWikiStatus.ts:196-212` already
does `.then(...).catch((error) => console.error('Failed to subscribe…', error))`
— the fix generalizes this instead of leaving one-off handling. **Its `.catch`
must be KEPT** (not swapped for a bare `.then` on a guarded promise — a fresh
`.then` chain re-creates an unhandled rejection, and keeping both guard+catch
double-logs).

**Test infra [V]:** Vitest 5, jsdom; `npm test` = `vitest run`. Global Tauri
mocks in `src/test-setup.ts` (lines 182-202: `listen` and `onDragDropEvent`
resolve with a no-op unlisten). PR #235's unit-test precedent:
`src/__tests__/safeUnlisten.test.ts` (plain unit tests, `vi.spyOn(console,'warn')`,
fake timers, no component mount).

## Root cause [V]

PR #235 hardened the *unlisten* half of the subscription lifecycle; the
*listen* half has no handler anywhere, so any registration rejection (destroyed
window, permission error, non-Tauri host such as a test harness) becomes a
browser `unhandledrejection` and the listener silently never exists. Impact is
low (issue's own assessment) — console noise plus a silently missing listener —
but it is the same hazard class PR #235 closed, on the other end of the promise.

## Proposed fix direction

One helper next to `safeUnlisten`, then mechanical call-site conversion:

```ts
export function guardListen(
  subscription: Promise<UnlistenFn>,
  context: string,
): Promise<UnlistenFn> {
  subscription.catch((err) => {
    console.warn(`[events] listen failed (${context})`, err);
  });
  return subscription; // same identity — existing safeUnlisten cleanup unchanged
}
```

- Wrap every subscription creation listed above with
  `guardListen(promise, "<event-name>")`. **Leak rule (Opus M2, corrected c2,
  site count corrected c3 — FOUR sites):** at the `Promise.all` sites
  (`useProviderHealth`, GenerationPanel, StepWatchItThink, AND
  StepFastembed), guard+catch alone leaves the already-subscribed listeners
  leaked when one rejects — convert those FOUR to hold the guarded PROMISES
  and clean up with `promises.forEach((p) => void safeUnlisten(p))` in
  cleanup regardless of resolution. (`safeUnlisten`'s signature accepts a
  SINGLE `UnlistenFn | Promise | undefined` — `events.ts:18-20` — NOT an
  array; the c1 draft's "pass the array" shape would not type-check. The
  forEach pattern is already the local idiom: `useProviderHealth.ts:85`,
  `GenerationPanel.tsx:51`, `StepWatchItThink.tsx:64`.)
- **Failure-behavior rules (Opus M3/M4, corrected c2):** subscriptions at the
  three Promise.all sites are set up with `Promise.allSettled` (NOT
  fail-fast `Promise.all` — a single rejection under fail-fast skips the
  remaining setup and leaves `unhandledrejection` + half-initialized UI, e.g.
  `StepFastembed.tsx:31,42` would never reach `initFastembed()`); each
  settled result that rejected is logged ONCE by guardListen (no extra
  `.catch` anywhere — double-log guard); `StepFastembed` calls
  `initFastembed()` after `allSettled` regardless of subscription outcomes
  (progress/error UI degrades, setup proceeds). **StepModel is its own case
  (c2 MODERATE):** moving its await inside `try` would abort the auto-install
  mid-flight (a progress-listener rejection must not flip the phase to
  error before `downloadSidecarEngine()` runs); StepModel keeps its
  ref-array cleanup with `await Promise.allSettled(unlistens.current)` —
  the allSettled is for cleanup awaiting, not setup gating. ModelPanel /
  StepOllama: `await` inside `try`, unlisten variable declared before
  `try`, `safeUnlisten` in `finally`; pull progress loss on rejection =
  proceed without progress display (pull itself is already in flight
  server-side; aborting the pull on a progress-listener failure would be
  worse).
- Convert `AppShell.tsx` drag-drop effect to the standard pattern:
  `const promise = guardListen(getCurrentWindow().onDragDropEvent(...), "drag-drop")`
  + cleanup `void safeUnlisten(promise)` — the `cancelled` flag dance becomes
  unnecessary (safeUnlisten already tolerates rejection).
- Normalize `useWikiStatus.ts` call sites to `guardListen` for consistent
  `[events]` warn formatting, KEEPING its existing `.catch` semantics
  (never a bare `.then` chain on a guarded promise — re-creates the
  unhandled rejection; never guard+catch on the same chain — double-logs).

**Tests (TDD):** guardListen rejects-logs-once-and-returns-identity; AppShell
component test — note `test-setup.ts:197-202` creates a FRESH
`onDragDropEvent` vi.fn per `getCurrentWindow()` call, so the test must
`mockReturnValueOnce` (or a shared mock), NOT `mockRejectedValueOnce` on a
stale reference; one warn, no unhandledrejection (vitest fails the suite on
unhandled rejections itself); useProviderHealth test with one of six
subscriptions rejecting → assert the other five unlisten fns still fire on
unmount (leak rule); StepFastembed test — subscription rejects AND
`initFastembed()` still runs (allSettled rule).

**Inventory cross-check (plan must include, Opus c2 NIT, command corrected
c3 — the original wildcard rg both errored on `--type tsx` and matched
non-subscriptions):** after conversion, enumerate subscription creators
EXPLICITLY and gate on adjacency, not emptiness:

```
rg -n --type ts "\b(listen|onVaultEvent|onPullProgress|onVaultSwitched|onSidecarDownloadProgress|onProviderLoading|onProviderReady|onProviderError|onEmbedInit|onGgufDownloadProgress|onIngest|onMigrationProgress|onMigrationComplete|onMigrationError|onConfigMalformed|onDragDropEvent|onWikiStatusChange)(<[^>]*>)?\(" src/ -g '!src/__tests__/**' -g '!src/lib/events.ts' -g '!src/test-setup.ts'
```

Gate: **every hit is the argument of a `guardListen(` call** (verify with
`rg -B1` over the same pattern) — a bare empty-result gate is unachievable
(the naive pattern matches `safeUnlisten(`, `requestAnimationFrame(`,
`function onScroll(`, etc.) and multi-line calls would be missed by a
same-line filter. Ripgrep notes: built-in `--type ts` already covers `*.tsx`;
`--type tsx` is not a type and aborts rg; the `(<[^>]*>)?\(` optional generic
bracket is REQUIRED — bare `listen\(` misses generic-parameterized calls like
`listen<PrivacyState>(...)` (usePrivacyMode.ts:83, SplashScreen.tsx:24/31,
controller-verified). KNOWN RESIDUAL GAP (controller-verified): a generic
bracket spanning lines (AppShell.tsx:234 `listen<{` with the payload type on
following lines) still evades the pattern — the plan's post-conversion check
must therefore ALSO diff against the doc's own inventory list (which includes
that site) rather than trust the grep alone. The pattern yields 27 hits at
the current head (2 of them comment lines — StepModel.tsx:70,
StepFastembed.tsx:19 — filtered by the adjacency gate); the plan re-runs it
and reconciles the count. If the wrapper list in events.ts grows a new `on*`
helper, this command's alternation must grow with it.

**Acceptance (from the issue):** a rejected listen/onDragDropEvent logs one
console.warn and produces no `unhandledrejection`; unit test covers a rejecting
subscription.

## Open questions

- **OQ1 (allSettled vs guard+catch — RESOLVED, revised c2):** subscriptions at
  the Promise.all sites (all four: useProviderHealth, GenerationPanel,
  StepWatchItThink, StepFastembed) use `Promise.allSettled` (c1's fail-fast
  wording was wrong: a rejected `Promise.all` both leaks the
  already-registered listeners AND skips remaining setup, leaving
  half-initialized UI); cleanup holds the guarded promise array and iterates
  `safeUnlisten` per entry — never a bare array argument, which
  `safeUnlisten`'s single-promise signature rejects.

## What was NOT checked

- Svelte/Solid analogues — none exist; the frontend is React-only.
- Whether `listen` wrappers in `events.ts` themselves should guard internally
  (alternative design: every `on*` wrapper returns a guarded promise). Rejected
  for now: it would double-log at sites that also handle rejection (useWikiStatus)
  and change wrapper return contracts mid-file; call-site wrapping is explicit
  and grep-able.
