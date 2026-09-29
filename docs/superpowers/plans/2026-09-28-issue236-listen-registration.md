# Tauri listener registration guards (issue #236) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** No frontend Tauri subscription can produce an `unhandledrejection`: a new `guardListen` helper logs registration failures exactly once, and every call site follows one of four per-site rules (held-promise cleanup, allSettled boot gate, pull-proceeds, existing-catch exemption).

**Architecture:** One helper in `src/lib/events.ts` (attaches a logging `.catch`, returns the ORIGINAL promise — existing `safeUnlisten` cleanup keeps working untouched); mechanical conversion of every subscription site per the spec's per-site rules; `Promise.all` sites become held-promise arrays (leak rule), setupWiki's four boot listeners get `Promise.allSettled` as a boot gate only, long-lived wiki sites keep held-promise cleanup.

**Tech Stack:** React/TS, vitest (+ @testing-library/react for the AppShell component test). No Rust changes.

**Spec:** `docs/superpowers/specs/2026-09-28-issue236-listen-registration-design.md` (companion investigation: `2026-09-28-issue236-listen-registration-investigation.md`; where the two disagree, THE SPEC WINS)

## Global Constraints

- `guardListen(subscription: Promise<UnlistenFn>, context: string): Promise<UnlistenFn>` — logs `[events] listen failed (<context>)` ONCE via `console.warn`, returns the same promise object.
- Exactly one warn per FAILED subscription (guardListen's own catch, or the exempted site's own `console.error`); successful subscriptions are silent. Never attach a second logging handler to the same promise.
- `safeUnlisten` takes a SINGLE `UnlistenFn | Promise<UnlistenFn> | undefined` — clean up arrays with `promises.forEach((p) => void safeUnlisten(p))`, never a bare array.
- Leak rule: at held-promise sites the cleanup holds the PROMISES (works even if `listen()` is still pending); settled-iteration cleanup is allowed ONLY in setupWiki (whose listeners are awaited before use).
- Pull rule (ModelPanel/StepOllama): the listener is created BEFORE `pullModel()` and its rejection must not abort the pull — progress becomes "unavailable", pull proceeds.
- `subscribeEntityStatus` + `useWikiStatus` are EXEMPT (their existing `.catch(console.error)` already handles rejection; guard+catch would double-log). Exemption is per call site.
- Run tests: `pnpm test` (vitest). Frontend only — `cargo` untouched.
- Conventional commits; all work on `fix/issue-236-listen-registration`, one PR (#247).

---

### Task 1: `guardListen` helper + unit tests

**Files:**
- Modify: `src/lib/events.ts` (after `safeUnlisten`, ~:51)
- Create: `src/__tests__/guardListen.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `export function guardListen(subscription: Promise<UnlistenFn>, context: string): Promise<UnlistenFn>` — every later task imports this.
- ALSO produces (review M1): `export type { UnlistenFn } from "@tauri-apps/api/event";` — events.ts imports `UnlistenFn` but does NOT re-export it today, so every later task's `import { …, type UnlistenFn } from './events'` fails `tsc` (TS2459) under `pnpm run build` (tsconfig covers all of src; vitest strips type imports so tests alone won't catch it). Add this export in Step 3 and import `UnlistenFn` FROM `./events` (not `@tauri-apps/api/event`) in Tasks 3, 6, 7 (useProviderHealth, GenerationPanel, StepWatchItThink, ModelPanel, StepOllama, wiki.ts).

- [ ] **Step 1: Write the failing tests**

```ts
// src/__tests__/guardListen.test.ts
import { describe, it, expect, vi, afterEach } from 'vitest';
import { guardListen, safeUnlisten, type UnlistenFn } from '../lib/events';

describe('guardListen', () => {
  afterEach(() => vi.restoreAllMocks());

  it('logs one warning with context when the subscription rejects', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const boom = new Error('listen failed');
    const p = guardListen(Promise.reject(boom), 'provider-loading');
    await p.catch(() => {});
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn).toHaveBeenCalledWith('[events] listen failed (provider-loading)', boom);
  });

  it('returns the SAME promise (identity preserved for cleanup)', () => {
    const inner: Promise<UnlistenFn> = Promise.resolve(() => {});
    expect(guardListen(inner, 'ctx')).toBe(inner);
  });

  it('never leaves an unhandled rejection even if nobody awaits the result', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    guardListen(Promise.reject(new Error('x')), 'ctx');
    await new Promise((r) => setTimeout(r, 0));
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it('stays silent when the subscription resolves', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await guardListen(Promise.resolve(() => {}), 'ctx');
    expect(warn).not.toHaveBeenCalled();
  });

  it('result still works with safeUnlisten after a rejection', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await expect(
      safeUnlisten(guardListen(Promise.reject(new Error('x')), 'ctx')),
    ).resolves.toBeUndefined();
    expect(warn).toHaveBeenCalledTimes(1); // guardListen logged; safeUnlisten swallowed
  });
});
```

- [ ] **Step 2: Run to verify RED**

Run: `pnpm test -- guardListen`
Expected: FAIL (`guardListen` is not exported).

- [ ] **Step 3: Implement** — add to `src/lib/events.ts` directly after `safeUnlisten`:

```ts
/**
 * Attach a logging rejection handler to a pending Tauri subscription so a
 * failed listen() surfaces as ONE console.warn instead of an
 * unhandledrejection. Returns the ORIGINAL promise — pass it to
 * safeUnlisten() exactly as before (safeUnlisten already tolerates a
 * rejected listen). Callers may add their OWN non-logging .catch for UI
 * degradation; never add a second LOGGING handler (double-log).
 */
export function guardListen(
  subscription: Promise<UnlistenFn>,
  context: string,
): Promise<UnlistenFn> {
  subscription.catch((err) => {
    console.warn(`[events] listen failed (${context})`, err);
  });
  return subscription;
}
```

- [ ] **Step 4: Run to verify GREEN**

Run: `pnpm test -- guardListen && pnpm test -- safeUnlisten`
Expected: ALL PASS (existing safeUnlisten tests untouched).

- [ ] **Step 5: Commit**

```bash
git add src/lib/events.ts src/__tests__/guardListen.test.ts
git commit -m "feat(events): guardListen logs subscription registration failures once (#236)"
```

### Task 2: Pattern-A sites (pending-promise cleanup) — hooks, SplashScreen, AppShell ×3

> **Line-number correction (review m3):** the original survey numbering is stale. Actual locations: useProviderHealth :47-81 (not :375-409), GenerationPanel :37-44 (not :437-445), StepWatchItThink :41-59 (not :477-498), StepModel :68-83 (not :593-608), StepOllama :24-44 (not :634-654), AppShell onVaultSwitched :134-146 / drag-drop :148-177 / config-malformed :234-243 (the draft's ":269-281 in current file" etc. was survey numbering, NOT file lines), startAutoHeal wiki.ts:545-552, startAutoMaintenance wiki.ts:590. Every edit below is anchored on quoted source text — locate edits by the quotes, never by line numbers.

**Files:**
- Modify: `src/hooks/usePrivacyMode.ts:83-91`
- Modify: `src/hooks/useVaultFiles.ts:15-18`
- Modify: `src/components/shell/SplashScreen.tsx:26-43`
- Modify: `src/components/shell/AppShell.tsx:134-146` (onVaultSwitched), `:148-177` (drag-drop), `:233-246` (config-malformed)

**Interfaces:**
- Consumes: `guardListen` (Task 1).
- Produces: nothing new — all sites keep their existing cleanup shape (`void safeUnlisten(promise)`).

- [ ] **Step 1: usePrivacyMode** — import `guardListen` from `../lib/events` and change

```ts
    const unlistenPromise = listen<PrivacyState>("privacy-mode-changed", (event) => {
      setState(event.payload);
    });
```

to

```ts
    const unlistenPromise = guardListen(
      listen<PrivacyState>("privacy-mode-changed", (event) => {
        setState(event.payload);
      }),
      "privacy-mode-changed",
    );
```

- [ ] **Step 2: useVaultFiles** — import `guardListen` and KEEP the direct `listen` import (the wrapped call below still uses it — dropping it breaks `tsc`; review m4):

```ts
    const unlisten = listen("vault-event", refresh);
```

to

```ts
    const unlisten = guardListen(listen("vault-event", refresh), "vault-event");
```

- [ ] **Step 3: SplashScreen** — wrap all three (:26-37):

```ts
    const unlistenProgress = guardListen(
      listen<MigrationProgressEvent>(
        "migration-progress",
        (event) => setProgress(event.payload),
      ),
      "migration-progress",
    );
    const unlistenComplete = guardListen(
      listen("migration-complete", () => {
        onComplete();
      }),
      "migration-complete",
    );
    const unlistenError = guardListen(
      listen<MigrationErrorEvent>(
        "migration-error",
        (event) => setError(event.payload.message),
      ),
      "migration-error",
    );
```

- [ ] **Step 4: AppShell onVaultSwitched** (:269-281 in current file) — wrap:

```ts
    const promise = guardListen(
      onVaultSwitched((newPath) => {
        setPeekTarget(null);
        setBrainEntityId(null);
        setBrainEntityName(null);
        setLibraryDoc(null);
        nav.reset({ mode: "brain" });
        onVaultChanged(newPath);
      }),
      "vault-switched",
    );
```

(cleanup `void safeUnlisten(promise)` unchanged).

- [ ] **Step 5: AppShell config-malformed** — wrap the pending promise (:336-342):

```ts
    const promise = guardListen(
      listen<{
        config_path: string;
        diagnostics: string[];
        remediation: string;
      }>("config-malformed", (event) => {
        renderMalformed(event.payload);
      }),
      "config-malformed",
    );
```

- [ ] **Step 6: AppShell drag-drop** — replace the WHOLE effect (:283-312, the `cancelled`-flag version) with the standard pattern:

```ts
  useEffect(() => {
    const promise = guardListen(
      getCurrentWindow().onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "leave") {
          setDragging(false);
          return;
        }
        if (payload.type === "enter" || payload.type === "over") {
          setDragging(true);
        } else if (payload.type === "drop") {
          setDragging(false);
        }
      }),
      "drag-drop",
    );
    return () => {
      void safeUnlisten(promise);
    };
  }, [vaultPath]);
```

(`safeUnlisten` already tolerates a rejected subscription — the `cancelled` dance is unnecessary.)

- [ ] **Step 7: Run the touched suites**

Run: `pnpm test`
Expected: ALL PASS (no behavior change for resolving subscriptions).

- [ ] **Step 8: Commit**

```bash
git add src/hooks/usePrivacyMode.ts src/hooks/useVaultFiles.ts src/components/shell/SplashScreen.tsx src/components/shell/AppShell.tsx
git commit -m "fix(ui): guard pending Tauri subscriptions in hooks and shell (#236)"
```

### Task 3: Held-promise leak fixes — useProviderHealth, GenerationPanel, StepWatchItThink

**Files:**
- Modify: `src/hooks/useProviderHealth.ts:44-83`
- Modify: `src/components/settings/GenerationPanel.tsx:26-53`
- Modify: `src/components/setup/StepWatchItThink.tsx:34-53`

**Interfaces:**
- Consumes: `guardListen` (Task 1).
- Produces: nothing new.

- [ ] **Step 1: useProviderHealth** — replace the entire `void Promise.all([...]).then(...)` block (:375-409 in survey numbering = the block from `void Promise.all([` through `});`) with a held-promise array; cleanup iterates it:

```ts
    const subscriptions = [
      guardListen(onProviderLoading(() => {
        if (active) setGeneration("loading");
      }), "provider-loading"),
      guardListen(onProviderReady(() => {
        if (!active) return;
        getProviderConfig()
          .then((cfg) => {
            if (active) {
              setGeneration(generationFromConfig(cfg.generation.provider));
            }
          })
          .catch(() => {
            if (active) setGeneration("error");
          });
      }), "provider-ready"),
      guardListen(onProviderError(() => {
        if (active) setGeneration("error");
      }), "provider-error"),
      guardListen(onEmbedInitProgress(() => {
        if (active) setEmbedding("loading");
      }), "embed-init-progress"),
      guardListen(onEmbedInitDone(() => {
        if (active) setEmbedding("ok");
      }), "embed-init-done"),
      guardListen(onEmbedInitError(() => {
        if (active) setEmbedding("error");
      }), "embed-init-error"),
    ];

    return () => {
      active = false;
      subscriptions.forEach((p) => void safeUnlisten(p));
    };
```

Delete the now-unused `unlisteners` array and its `push` mechanism (cleanup previously iterated resolved fns only — that is the leak). Type note: `subscriptions` is `Array<Promise<UnlistenFn>>`.

- [ ] **Step 2: GenerationPanel** — in `setup()`, replace the awaited `Promise.all` + assignment (:437-445):

```ts
      const subscriptions = [
        guardListen(onProviderLoading(() => setStatus("loading")), "provider-loading"),
        guardListen(onProviderReady(() => {
          setStatus(cfg?.generation.provider === "unconfigured" ? "unconfigured" : "ready");
        }), "provider-ready"),
        guardListen(onProviderError(() => setStatus("error")), "provider-error"),
      ];
      unlistens = subscriptions;
```

and re-type the outer `unlistens` declaration to hold promises (`let unlistens: Array<Promise<UnlistenFn>> = []`). The effect cleanup (`unlistens.forEach(...)`) is otherwise unchanged — `safeUnlisten` accepts pending promises. (Review m5: do NOT add the old draft's `if (!active) unlistens.forEach(...)` — `active` was already checked right after the await at GenerationPanel.tsx:28, so the check can never be true and cleanup already covers the pending promises.)

- [ ] **Step 3: StepWatchItThink** — same shape; replace the awaited `Promise.all` IIFE body (:477-498):

```ts
    (async () => {
      unlistens = [
        guardListen(onIngestProgress((p) => {
          if (!mounted) return;
          lastProgressAt.current = Date.now();
          applyPhase(p.phase);
        }), "ingest-progress"),
        guardListen(onIngestProposalReady((p) => {
          if (!mounted) return;
          lastProgressAt.current = Date.now();
          setProposalId(p.proposalId);
          applyPhase("ready");
        }), "ingest-proposal-ready"),
        guardListen(onIngestError((p) => {
          if (!mounted) return;
          setErrorMsg(p.message);
          applyPhase("error");
        }), "ingest-error"),
      ];
      if (!mounted) unlistens.forEach((p) => void safeUnlisten(p));
    })();
```

re-type `unlistens` to `Array<Promise<UnlistenFn>>`; cleanup `unlistens.forEach((u) => void safeUnlisten(u))` unchanged.

- [ ] **Step 4: Failing-first regression tests (review M4; spec Testing :115-121).** The behavior changes in this task (leak fix) ship with no coverage unless added here. For each component, a test where ONE of its subscriptions rejects and the OTHERS still unlisten on unmount:

```ts
// Per component (useProviderHealth, GenerationPanel, StepWatchItThink):
// 1. Mock events so, say, onProviderLoading rejects and the rest resolve.
// 2. Render the component (hook: renderHook).
// 3. Assert exactly one '[events] listen failed (provider-loading)' warn.
// 4. Unmount.
// 5. Assert EVERY subscription's unlisten fn was invoked — including the
//    ones whose listen() rejected (safeUnlisten tolerates them). That is
//    the leak fix: the old Promise.all shape dropped the pending promises.
```

- [ ] **Step 5: Run**

Run: `pnpm test`
Expected: ALL PASS, including the new regression tests (RED first in Step 4's draft form only while the components still use `Promise.all`).

- [ ] **Step 6: Commit**

```bash
git add src/hooks/useProviderHealth.ts src/components/settings/GenerationPanel.tsx src/components/setup/StepWatchItThink.tsx
git commit -m "fix(ui): hold pending subscription promises so one rejection cannot leak the rest (#236)"
```

### Task 4: StepFastembed — allSettled boot gate (initFastembed always runs)

**Files:**
- Modify: `src/components/setup/StepFastembed.tsx:29-56`

- [ ] **Step 1: Rewrite the effect body** — wrap both subscriptions in `guardListen`, swap `Promise.all` for `Promise.allSettled` (a rejected subscription must NOT skip `initFastembed`):

```ts
  useEffect(() => {
    let mounted = true;
    // Hold the pending subscriptions so cleanup can remove them even when the
    // step unmounts before listen() resolves.
    const unlistenDone = guardListen(onEmbedInitDone(() => {
      if (!mounted) return;
      onNext();
    }), "embed-init-done");
    const unlistenError = guardListen(onEmbedInitError(({ message }) => {
      if (!mounted) return;
      setErrorMsg(message);
      setPhase("error");
    }), "embed-init-error");

    const setup = async () => {
      // allSettled: subscription failure degrades progress/error display but
      // must never skip the actual init (the old Promise.all skipped it).
      await Promise.allSettled([unlistenDone, unlistenError]);

      try {
        await initFastembed();
      } catch (err) {
        if (!mounted) return;
        setErrorMsg(String(err));
        setPhase("error");
      }
    };

    setup();
    return () => {
      mounted = false;
      void safeUnlisten(unlistenDone);
      void safeUnlisten(unlistenError);
    };
  }, [onNext]);
```

(Import `guardListen` alongside the existing events imports.)

- [ ] **Step 2: Failing-first test (review M4; spec Testing :117-118):** mock `onEmbedInitDone` to REJECT and assert `initFastembed()` was still called exactly once (the old `Promise.all` shape skipped it), and that exactly one `[events] listen failed (embed-init-done)` warn fired. RED while the component still uses `Promise.all`.

- [ ] **Step 3: Run**

Run: `pnpm test`
Expected: ALL PASS.

- [ ] **Step 4: Commit**

```bash
git add src/components/setup/StepFastembed.tsx
git commit -m "fix(setup): fastembed init proceeds when event subscriptions reject (#236)"
```

### Task 5: StepModel — pre-install drain

**Files:**
- Modify: `src/components/setup/StepModel.tsx:593-608`

- [ ] **Step 1: Wrap + allSettled** — in `runAutoInstall`, replace the subscription block and its await:

```ts
    cleanup();
    // Store the pending subscriptions before draining them so cleanup can
    // remove them even when the step unmounts before listen() resolves.
    unlistens.current = [
      guardListen(onSidecarDownloadProgress(({ downloaded, total }) => {
        setProgress(total > 0 ? Math.round((downloaded / total) * 100) : 0);
      }), "sidecar-download-progress"),
      guardListen(onGgufDownloadProgress(({ downloaded, total }) => {
        setProgress(total > 0 ? Math.round((downloaded / total) * 100) : 0);
      }), "gguf-download-progress"),
      guardListen(onProviderError(({ message }) => {
        setErrorMsg(message);
        setPhase("auto-error");
      }), "provider-error"),
    ];
    // Pre-install drain (NOT teardown): make sure registration outcomes are
    // settled before the downloads start; a rejected subscription degrades
    // progress display but must NOT abort the install.
    await Promise.allSettled(unlistens.current);
```

(`cleanup()`/`unlistens.current` sweep remains the teardown — it already passes pending promises to `safeUnlisten`.)

- [ ] **Step 2: Run**

Run: `pnpm test`
Expected: ALL PASS.

- [ ] **Step 3: Commit**

```bash
git add src/components/setup/StepModel.tsx
git commit -m "fix(setup): model auto-install proceeds when progress subscriptions reject (#236)"
```

### Task 6: ModelPanel + StepOllama — pull proceeds when its listener rejects

**Files:**
- Modify: `src/components/settings/ModelPanel.tsx:35-56` (state + `handlePull`)
- Modify: `src/components/setup/StepOllama.tsx:34-54` (`pull`)

- [ ] **Step 1: ModelPanel** — add a display flag next to the existing progress state (find `const [progress, setProgress] = ...` near the top of the component) :

```tsx
  const [progressUnavailable, setProgressUnavailable] = useState(false);
```

Rewrite `handlePull`:

```tsx
  async function handlePull() {
    if (!newModel.trim()) return;
    setPhase("pulling");
    setProgress(0);
    setProgressUnavailable(false);
    setError(null);
    // Progress listener is best-effort: if it cannot attach, show that and
    // STILL pull — aborting the user's pull over a display listener would be
    // worse. guardListen logs the failure; no second logger here.
    const unlisten = guardListen(
      onPullProgress(({ completed, total }) => {
        setProgress(total > 0 ? Math.round((completed / total) * 100) : 0);
      }),
      "ollama-pull-progress",
    );
    // Review M5: AWAIT (not `void`) — Tauri does not buffer events, so
    // pullModel() must not start until registration settles; early
    // ollama-pull-progress events would otherwise be lost and a fast/cached
    // pull could finish before the listener exists (progress stuck at 0%).
    // A rejection still lets the pull proceed (degraded display only).
    await unlisten.catch(() => setProgressUnavailable(true));
    try {
      await pullModel(newModel.trim());
      setPhase("done");
      const updated = await listLocalModels();
      setModels(updated);
      setNewModel("");
    } catch (e) {
      setError(String(e));
      setPhase("error");
    } finally {
      void safeUnlisten(unlisten);
    }
  }
```

In the JSX progress row, render the degraded state (locate the progress render inside `phase === "pulling"` / done block and add):

```tsx
      {progressUnavailable && <p className="settings-hint">Progress unavailable — pull continuing.</p>}
```

- [ ] **Step 2: StepOllama** — same shape in `pull` (:634-654); add `const [progressUnavailable, setProgressUnavailable] = useState(false);` beside its progress state, reset it at pull start, and replace the listener block:

```tsx
    setPhase("pulling");
    setProgress(0);
    setProgressUnavailable(false);
    // Best-effort progress: rejection degrades display, never aborts the pull.
    const unlisten = guardListen(
      onPullProgress(({ completed, total }) => {
        setProgress(total > 0 ? Math.round((completed / total) * 100) : 0);
      }),
      "ollama-pull-progress",
    );
    // Same await-before-pull as ModelPanel (review M5).
    await unlisten.catch(() => setProgressUnavailable(true));
    try {
      await pullModel(modelId);
      setPhase("ready");
    } catch (e) {
      setErrorMsg(String(e));
      setPhase("error");
    } finally {
      void safeUnlisten(unlisten);
    }
```

plus the same hint line in its pulling-phase JSX.

- [ ] **Step 3: Failing-first tests (review M4; spec Testing :119-120):** for ModelPanel and StepOllama — mock `onPullProgress` to REJECT and `pullModel` to resolve; assert the pull still ran (phase reaches done/ready), the "Progress unavailable" hint rendered, and exactly one `[events] listen failed (ollama-pull-progress)` warn fired. RED while the listener still aborts/skips the pull.

- [ ] **Step 4: Run**

Run: `pnpm test`
Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add src/components/settings/ModelPanel.tsx src/components/setup/StepOllama.tsx
git commit -m "fix(ui): model pulls proceed when their progress listener rejects (#236)"
```

### Task 7: wiki.ts — setupWiki boot gate + long-lived held-promise sites

**Files:**
- Modify: `src/lib/wiki.ts` (setupWiki :449-465, startAutoHeal :552-569, startAutoMaintenance :597-606, module teardown of the setupWiki listeners)

**Interfaces:**
- Consumes: `guardListen` (Task 1).
- Produces: module-level `wikiLifecycleListeners: Array<Promise<UnlistenFn>>`. (Review m1: wiki.ts:495-499 today only does `void startedUnlisten; …` with a "listeners live for the session" comment — there is NO existing teardown path to consume this array. Step 2 therefore DELETES those `void` lines instead of replacing a disposal that doesn't exist; the array serves as the allSettled gate's handle, not a teardown registry.)

- [ ] **Step 1: setupWiki boot gate.** Add a module-level array near the other module state (top of file, beside `_ontologySelection` etc.):

```ts
// Pending setupWiki lifecycle subscriptions. Held as PROMISES so teardown
// works even if disposal runs while a listen() is still registering.
const wikiLifecycleListeners: Array<Promise<UnlistenFn>> = [];
```

(Import `UnlistenFn` type + `guardListen` from `./events` — note wiki.ts currently imports `listen` from `@tauri-apps/api/event`.)

Replace the four awaits (:450-464 region):

```ts
  wikiLifecycleListeners.push(
    guardListen(listen<void>('outbox-worker-started', async () => {
      _outboxEnabled = true;
      await rebuildWiki();
    }), 'outbox-worker-started'),
    guardListen(listen<void>('outbox-worker-stopped', async () => {
      _outboxEnabled = false;
      await rebuildWiki();
    }), 'outbox-worker-stopped'),
    guardListen(listen<void>('classifier-config-changed', onClassifierInputsChanged), 'classifier-config-changed'),
    guardListen(listen<void>('privacy-mode-changed', onClassifierInputsChanged), 'privacy-mode-changed'),
  );
  // Boot gate (allSettled — never fail boot over a subscription): outcomes
  // are settled before the initial setup runs, preserving the
  // register-before-setup intent. A rejected listener degrades its feature
  // (worker-lifecycle rebuilds / classifier refreshes) while the wiki engine
  // still comes up.
  await Promise.allSettled(wikiLifecycleListeners);
```

Delete the four `const startedUnlisten = ...` / `stoppedUnlisten` / `classifierUnlisten` / `privacyUnlisten` bindings.

- [ ] **Step 2: Delete the `void` disposal lines (review m1).** wiki.ts:495-499 has only `void startedUnlisten; void stoppedUnlisten; …` ("listeners live for the session") — there is NO disposal to replace. Delete those lines and the comment; do NOT grep for a teardown path that does not exist. The listeners intentionally live for the session; `wikiLifecycleListeners` exists as the Step 1 allSettled gate's handle, not a teardown registry.

- [ ] **Step 3: startAutoHeal** (:552-568) — wrap and KEEP held-promise cleanup (its stop path can run while `listen()` is pending):

```ts
  const subscriptions = [
    guardListen(
      listen<VaultEventPayload>('vault-event', (event) => {
        if (!active) return;
        if (event.payload.kind === 'Deleted') {
          scheduleHeal();
        }
      }),
      'vault-event (auto-heal)',
    ),
  ];
```

and in the returned cleanup: `subscriptions.forEach((p) => void safeUnlisten(p));`

- [ ] **Step 4: startAutoMaintenance** (:597) — same:

```ts
  const subscriptions = [
    guardListen(listen('wiki-status-change', handleStatusChange), 'wiki-status-change (auto-maintenance)'),
  ];
```

cleanup: `subscriptions.forEach((p) => void safeUnlisten(p));`

- [ ] **Step 5: useWikiStatus / subscribeEntityStatus — NO CHANGE.** The hook's existing `.catch(console.error)` already owns rejection handling (spec exemption). Do not touch `src/hooks/useWikiStatus.ts` or `subscribeEntityStatus` in `src/lib/tauri.ts`.

- [ ] **Step 6: Failing-first test (review M4; spec Testing :120-121):** for setupWiki — mock `listen` so ONE of the four boot listeners rejects; assert `setupWiki()` still completes (boot gate holds) and exactly one `[events] listen failed (<ctx>)` warn fired. RED while setupWiki still uses the old shape.

- [ ] **Step 7: Run**

Run: `pnpm test`
Expected: ALL PASS.

- [ ] **Step 8: Commit**

```bash
git add src/lib/wiki.ts
git commit -m "fix(wiki): boot gate + held-promise cleanup for wiki event subscriptions (#236)"
```

### Task 8: AppShell component test + inventory cross-check + spec flip

**Files:**
- Create: `src/__tests__/AppShell.dragdrop.test.tsx` (in `src/__tests__/`, NOT beside the component — see Step 1)
- Modify: `docs/superpowers/specs/2026-09-28-issue236-listen-registration-design.md` (status + the deliberate departures, review m2)

- [ ] **Step 1: Write the AppShell rejection test.** (Review M2 — the old draft's test could not pass: it captured its own `getCurrentWindow()` object, but `test-setup.ts:197-202` returns a FRESH object with a fresh `vi.fn` per call, so the component got a resolving mock and no warn fired; `render(<AppShell />)` omitted AppShell's REQUIRED props (`AppShell.tsx:61`: `{ vaultPath, onVaultChanged, needsSetup }`); and the Proxy mock of `lib/tauri` made `needsChunkHashMigration()`/`startFileWatcher()` return `undefined`, whose `.then`/`.catch` throw during mount — the Proxy could even answer `then` and look like a thenable. This rewrite fixes all four.) The file goes in `src/__tests__/` — NOT `src/components/shell/__tests__/` — because tsconfig excludes only `src/__tests__` and this file's type surface would otherwise break `pnpm run build`:

```tsx
// src/__tests__/AppShell.dragdrop.test.tsx
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/react';
import { getCurrentWindow } from '@tauri-apps/api/window';

// Explicit resolving stubs for exactly what AppShell + its hooks call at
// mount. NO Proxy — a Proxy mock answers `then` (thenable trap) and returns
// `undefined` for needsChunkHashMigration/startFileWatcher, whose .then
// calls throw during mount.
vi.mock('../lib/tauri', () => ({
  needsChunkHashMigration: vi.fn(() => Promise.resolve(false)),
  startFileWatcher: vi.fn(() => Promise.resolve(() => {})),
  // add stubs for anything else the mount trace touches — extend this
  // object, never switch to a Proxy.
}));

import { AppShell } from '../components/shell/AppShell';

describe('AppShell drag-drop registration failure', () => {
  beforeEach(() => vi.clearAllMocks());

  it('logs exactly one warning and does not throw unhandled when onDragDropEvent rejects', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    // Mock at the SOURCE (getCurrentWindow), not on a captured object —
    // test-setup returns a fresh object per call, so per-object
    // mockReturnValueOnce never reaches the component.
    vi.mocked(getCurrentWindow).mockImplementation(() => ({
      onDragDropEvent: vi.fn(() => Promise.reject(new Error('no drag drop'))),
      setTitle: vi.fn(() => Promise.resolve()),
    }) as never);

    render(
      <AppShell
        vaultPath="/v"
        onVaultChanged={() => {}}
        needsSetup={false}
      />,
    );
    await new Promise((r) => setTimeout(r, 0));

    // Review m6: assert the COUNT on warns filtered by the prefix — a bare
    // toHaveBeenCalledWith does not prove "exactly one", and the
    // config-malformed drain can also warn (AppShell.tsx:232).
    const eventsWarns = warn.mock.calls.filter(([m]) =>
      String(m).startsWith('[events] listen failed'),
    );
    expect(eventsWarns).toHaveLength(1);
    expect(eventsWarns[0][0]).toBe('[events] listen failed (drag-drop)');
    expect(eventsWarns[0][1]).toBeInstanceOf(Error);
    warn.mockRestore();
  });
});
```

- [ ] **Step 2: Run**

Run: `pnpm test -- AppShell`
Expected: PASS (one warn; no unhandled rejection — vitest fails the run otherwise).

- [ ] **Step 3: Inventory cross-check.** Every subscription creation in `src/` must now be either guardListen-wrapped or covered by an explicit rule (setupWiki allSettled, startAutoHeal/startAutoMaintenance held-promise, useWikiStatus existing-catch, pull-proceeds sites):

```bash
# Review M3: the old pattern required `onEmbedInit`/`onIngest` to be followed
# DIRECTLY by `<` or `(`, so it silently missed onEmbedInitDone/Progress/Error
# and onIngestProgress/ProposalReady/Error (useProviderHealth.ts:66-72,
# StepFastembed.tsx:20,24, StepWatchItThink.tsx:42-53). It also listed
# non-existent wrappers (onMigration*, onConfigMalformed) and omitted
# subscribeEntityStatus. Wildcards fix the misses:
rg -n --type ts "\b(listen|subscribeEntityStatus|on(VaultEvent|PullProgress|VaultSwitched|SidecarDownloadProgress|Provider\w+|EmbedInit\w+|GgufDownloadProgress|Ingest\w+|DragDropEvent))(<[^>]*>)?\(" src/ -g '!src/__tests__/**' -g '!src/lib/events.ts' -g '!src/test-setup.ts'
```

Gate: every hit is an argument of `guardListen(` (check with `rg -B1` over the same pattern), or sits in setupWiki/startAutoHeal/startAutoMaintenance (which wrap internally), or is `subscribeEntityStatus`'s own body (exempt). EXPECTED COUNT: ~34 hits at head including 2 comment lines — if the count differs, diff the hit list against this explicit site list and resolve EVERY delta by eye before committing; do not proceed with unexplained hits. Known grep gap (do not hand-roll around it): multi-line generics (AppShell config-malformed uses `listen<{` across lines — verify that one by eye at :233-246). Record the checked hit list in the commit message body.

- [ ] **Step 4: Full verification**

Run: `pnpm test && pnpm run build`
Expected: all green.

- [ ] **Step 5: Flip spec status + commit + push**

Set `**Status:**` to `Implemented 2026-09-28 (PR #247)` in the design doc, AND amend the spec to record the plan's deliberate departures (review m2 — the plan diverges from the spec in three places and claims "THE SPEC WINS", so the spec must be updated to match reality):

1. **Item 3 (await vs allSettled):** useProviderHealth, GenerationPanel and StepWatchItThink use held-promise arrays WITHOUT awaiting settlement, instead of the spec's `allSettled` — nothing at those sites needs the registration outcome, so awaiting would only delay unmount-path cleanup.
2. **Item 7 (cleanup holds promises vs iterating settled results):** setupWiki cleanup keeps held promises (`safeUnlisten` tolerates pending), rather than iterating settled results.
3. **Line 110 (test location):** guardListen tests live in a new `src/__tests__/guardListen.test.ts` rather than extending `safeUnlisten.test.ts`.

Then:

```bash
git add src/__tests__/AppShell.dragdrop.test.tsx docs/superpowers/specs/2026-09-28-issue236-listen-registration-design.md
git commit -m "test(ui): drag-drop rejection guard test; mark #236 design implemented (PR #247)"
git push origin fix/issue-236-listen-registration
```
