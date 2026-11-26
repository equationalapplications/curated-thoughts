# UnverifiedPromptModal focus-flake fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the focus-timing races in `src/__tests__/UnverifiedPromptModal.test.tsx` by waiting for the trap's passive effect before the three racy assertions, then prove the fix with a quantified stability bar.

**Architecture:** Test-only change. The focus trap registers its handler and moves focus in a passive effect; under load, tests interacted with the dialog before that effect flushed. Three `waitFor` wraps make the tests wait for the effect instead of racing it. No product code changes.

**Tech Stack:** Vitest + React Testing Library (`waitFor` is already imported in the file), pnpm, happy-dom.

**Spec:** `specs/issue-386-focus-flake/spec.md` (this plan argues from the spec; read both).

## Global Constraints

- No product code changes. Only `src/__tests__/UnverifiedPromptModal.test.tsx` is modified (spec, Files touched).
- Synchronization must use `waitFor` only. No sleeps, no fake timers, no custom timeouts (spec, Rejected alternatives).
- `npx prettier --check` must pass on every touched file (repo CI format gate).
- Commit messages use conventional prefixes (`test:`, `docs:`), no `!`, no `[skip ci]` (repo release rules).
- Run pnpm, not npm. Snap-node `…/bin/env: No such file or directory` stderr lines are noise; judge by the vitest output, not stderr.
- Never leave uncommitted tree state between dispatches.
- Stability-bar numbers are exact: 30 loaded runs + 20 unloaded runs + one full-suite run, all green (spec, Test plan).

---

### Task 1: Add the three `waitFor` wraps

**Files:**

- Modify: `src/__tests__/UnverifiedPromptModal.test.tsx` (lines 138-142, 185-190, 213-216 at plan-writing time; anchor on exact code, not line numbers)

**Interfaces:**

- Consumes: `waitFor` from `@testing-library/react` (already imported; the file uses it at lines 171 and 213).
- Produces: no exports. Later tasks rely on the file passing `pnpm vitest run src/__tests__/UnverifiedPromptModal.test.tsx` with 9/9 tests green.

- [ ] **Step 1: Fix 1 — wrap the focus-on-open assertion**

Find this block (test "focus moves into the dialog on open"):

```tsx
  await screen.findByText("Real body from the brain.");
  expect(document.activeElement).toBe(
    screen.getByRole("button", { name: /^confirm$/i }),
  );
});
```

Replace with:

```tsx
  await screen.findByText("Real body from the brain.");
  await waitFor(() => {
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: /^confirm$/i }),
    );
  });
});
```

- [ ] **Step 2: Fix 2 — wait for initial focus before firing Tab**

Find this block (test "Tab from the last focusable wraps to the first focusable inside the dialog"):

```tsx
expect(last).toHaveTextContent(/not now/i);
last.focus();
```

Replace with:

```tsx
expect(last).toHaveTextContent(/not now/i);
// The trap's passive effect must have run before we fire Tab, or its
// keydown handler is not registered yet. Wait for its initial focus.
await waitFor(() => {
  expect(document.activeElement).toBe(
    screen.getByRole("button", { name: /^confirm$/i }),
  );
});
last.focus();
```

- [ ] **Step 3: Fix 3 — wrap the close-test focus-restore assertion**

Find this block (test "on close, focus returns to the previously focused element"):

```tsx
  await waitFor(() => {
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  expect(document.activeElement).toBe(opener);
});
```

Replace with:

```tsx
  await waitFor(() => {
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  // The focus restore runs in the trap's effect cleanup, another passive
  // flush that can lag the DOM commit under load. Wait for it too.
  await waitFor(() => {
    expect(document.activeElement).toBe(opener);
  });
});
```

- [ ] **Step 4: Run the file**

Run: `pnpm vitest run src/__tests__/UnverifiedPromptModal.test.tsx 2>&1 | tail -8`
Expected: `Tests  9 passed (9)` and `Test Files  1 passed (1)`. Read the real output; do not assume.

- [ ] **Step 5: Format check and commit**

Run: `npx prettier --check src/__tests__/UnverifiedPromptModal.test.tsx`
Expected: "All matched files use Prettier code style!". If it fails, run `npx prettier --write` on that one file and re-run Step 4.

```bash
git add src/__tests__/UnverifiedPromptModal.test.tsx
git commit -m "test: #386 — wait for the focus trap's passive effect before asserting"
```

### Task 2: Keyboard-site sweep (owner sanity check)

**Files:**

- None modified. Verification only.

**Interfaces:**

- Consumes: nothing. Produces: a ledger evidence line confirming no other keyboard test races the trap.

The owner's approval (Discord msg 1575535367145398364) requires: "confirm no other keyboard test in UnverifiedPromptModal.test.tsx (such as Shift+Tab or Escape) fires a keyDown immediately after findByText without waiting for the trap effect."

- [ ] **Step 1: Enumerate every keyboard interaction site**

Run: `grep -n 'keyDown\|keyboard\|Escape\|fireEvent' src/__tests__/UnverifiedPromptModal.test.tsx`
Expected at plan-writing time: exactly one `fireEvent.keyDown` site (the Tab test, now behind the Fix 2 `waitFor`), plus `fireEvent.click` sites. No Shift+Tab or Escape keydown tests exist. Pre-plan evidence (2026-11-26, commit 53baa084): the only keyDown was line 192; click sites at lines 90, 101, 170, 212 reach React props set at commit time and do not race the trap.

- [ ] **Step 2: Record the result in LEDGER.md**

Tick Task 2's box with the grep output summary as evidence. If the grep shows a NEW racy keyboard site the plan did not anticipate, STOP and report — do not improvise a fix outside the spec.

### Task 3: Stability bar

**Files:**

- Modify: `specs/issue-386-focus-flake/spec.md` (Errata section only)

**Interfaces:**

- Consumes: Task 1's committed test change. Produces: recorded evidence that the fix holds under load.

- [ ] **Step 1: Loaded loop — 30 runs**

Spawn 6 spinner processes on the 4-core machine, run the file 30 times, count results. Track the spinner PIDs in a variable and kill only those (a `pkill -f 'while :'` would kill the running shell itself):

```bash
N=$(nproc); SPIN=""
for i in $(seq 1 $((N+2))); do (while :; do :; done) & SPIN="$SPIN $!"; done
LOADED=$(for i in $(seq 1 30); do pnpm vitest run src/__tests__/UnverifiedPromptModal.test.tsx 2>&1 | grep -c 'Tests  9 passed'; done | grep -c 1)
kill $SPIN
echo "loaded green: $LOADED/30"
```

Expected: `loaded green: 30/30`. Kill the spinners via `$SPIN` even if a run fails. Judge each iteration by the `Tests  9 passed` line.

- [ ] **Step 2: Unloaded loop — 20 runs**

```bash
UNLOADED=$(for i in $(seq 1 20); do pnpm vitest run src/__tests__/UnverifiedPromptModal.test.tsx 2>&1 | grep -c 'Tests  9 passed'; done | grep -c 1)
echo "unloaded green: $UNLOADED/20"
```

Expected: `unloaded green: 20/20`.

- [ ] **Step 3: Full frontend suite**

Run: `pnpm vitest run 2>&1 | tail -6`
Expected: every test file passes and the totals line shows 0 failed. Read the real totals; do not assume.

- [ ] **Step 4: Record results in the spec Errata and commit**

Fill `specs/issue-386-focus-flake/spec.md`'s Errata section with: the loaded count over 30, the unloaded count over 20, the full-suite totals, the machine profile (4 cores, 6 spinner processes), and the date.

```bash
git add specs/issue-386-focus-flake/spec.md
git commit -m "docs(spec): #386 — record stability-bar results in Errata"
```

### Task 4: Cleanup and ready-flip (run only after T4 review APPROVE)

**Files:**

- Delete: `specs/issue-386-focus-flake/plan.md`, `specs/issue-386-focus-flake/LEDGER.md`
- Modify: `specs/issue-386-focus-flake/spec.md` (status line only)

**Interfaces:**

- Consumes: Tasks 1-3 complete and the T4 review verdict APPROVE. Produces: the ready-for-review PR state.

- [ ] **Step 1: Flip the spec status**

Change the spec's status line to:
`Status: Implemented (2026-11-26 — PR #387; stability bar in Errata)`

- [ ] **Step 2: Delete transient scaffolding and mark the PR ready**

```bash
git rm specs/issue-386-focus-flake/plan.md specs/issue-386-focus-flake/LEDGER.md
git add specs/issue-386-focus-flake/spec.md
git commit -m "docs(spec): #386 — close out; Implemented; prune plan and ledger"
gh pr ready 387
```

Expected: PR #387 state becomes READY FOR REVIEW (verify with `gh pr view 387 --json isDraft,state`).
