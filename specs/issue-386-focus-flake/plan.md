# UnverifiedPromptModal focus-flake fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the focus-timing races in `src/__tests__/UnverifiedPromptModal.test.tsx` by waiting for the trap's passive effect before the three racy assertions, then prove the fix with a quantified stability bar.

**Architecture:** Test-only change. The focus trap registers its handler and moves focus in a passive effect; under load, tests interacted with the dialog before that effect flushed. Three `waitFor` wraps make the tests wait for the effect instead of racing it. No product code changes.

**Tech Stack:** Vitest + React Testing Library (`waitFor` is already imported in the file), pnpm, jsdom 31 (`vite.config.ts:91`).

**Spec:** `specs/issue-386-focus-flake/spec.md` (this plan argues from the spec; read both).

## Global Constraints

- No product code changes. Only `src/__tests__/UnverifiedPromptModal.test.tsx` is modified (spec, Files touched).
- Synchronization must use `waitFor` only. No sleeps, no fake timers, no custom timeouts (spec, Rejected alternatives).
- This repo has NO Prettier gate and NO Prettier config. Never run Prettier write-mode on the test file (it would reflow unrelated lines, e.g. the 86-character line 148). The gates are: `npx eslint src/__tests__/UnverifiedPromptModal.test.tsx` clean, and `git diff --stat` after the edit showing exactly one file, with changes confined to the three sites.
- Commit messages use conventional prefixes (`test:`, `docs:`), no `!`, no `[skip ci]` (repo release rules).
- Run pnpm, not npm. Snap-node `…/bin/env: No such file or directory` stderr lines are noise; judge by the vitest output, not stderr.
- Never leave uncommitted tree state between dispatches.
- Stability-bar numbers are exact and set by the owner's plan ruling: loaded runs + 20 unloaded runs + one full-suite run, all green (spec, Test plan).

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

Find this block (test "Tab from the last focusable wraps to the first focusable inside the dialog"; note the 2-space indentation, which the exact-string edit must preserve):

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

- [ ] **Step 3: Fix 3 — wrap the close-test focus-restore assertion (DEFENSIVE; owner decides keep-or-drop at plan approval)**

If the owner drops Fix 3 at plan approval: skip this step, and change "three" to "two" in the spec's Files touched (spec.md), the plan's Goal line, this task's intro, and the LEDGER task count.

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
  // Defensive: the restore runs in the trap's effect cleanup, another
  // passive flush. No failure of this test was ever captured, but the
  // wrap is free insurance and cannot mask a broken restore.
  await waitFor(() => {
    expect(document.activeElement).toBe(opener);
  });
});
```

- [ ] **Step 4: Run the file**

Run: `pnpm vitest run src/__tests__/UnverifiedPromptModal.test.tsx 2>&1 | tail -8`
Expected: `Tests  9 passed (9)` and `Test Files  1 passed (1)`. Read the real output; do not assume.

- [ ] **Step 5: Lint, scope check, and commit**

Run: `npx eslint src/__tests__/UnverifiedPromptModal.test.tsx`
Expected: no errors. Then run `git diff --stat` and confirm it lists exactly one file and `git diff` shows changes only at the three sites. This repo has no Prettier gate; do not run Prettier on the test file.

```bash
git add src/__tests__/UnverifiedPromptModal.test.tsx
git commit -m "test: #386 — wait for the focus trap's passive effect before asserting"
```

### Task 2: Keyboard-site sweep (owner sanity check)

**Files:**

- Modify: `specs/issue-386-focus-flake/spec.md` (Errata only), `specs/issue-386-focus-flake/LEDGER.md` (evidence tick)

**Interfaces:**

- Consumes: nothing. Produces: a ledger evidence line confirming no other keyboard test races the trap.

The owner's approval (Discord msg 1575535367145398364) requires: "confirm no other keyboard test in UnverifiedPromptModal.test.tsx (such as Shift+Tab or Escape) fires a keyDown immediately after findByText without waiting for the trap effect."

- [ ] **Step 1: Enumerate every keyboard interaction site**

Run: `grep -n 'keyDown\|keyboard\|Escape\|fireEvent' src/__tests__/UnverifiedPromptModal.test.tsx`
Expected at plan-writing time: exactly one `fireEvent.keyDown` site (the Tab test, now behind the Fix 2 `waitFor`), plus `fireEvent.click` sites. No Shift+Tab or Escape keydown tests exist. Pre-plan evidence (2026-11-26, commit 53baa084): the only keyDown was line 192; click sites at lines 90, 101, 170, 212 reach React props set at commit time and do not race the trap.

- [ ] **Step 2: Record the result in LEDGER.md and the spec Errata**

Tick Task 2's box with the grep output summary as evidence, and record the same one-line result in the spec's Errata section (LEDGER.md is deleted at cleanup in Task 4; Errata is the durable home). If the grep shows a NEW racy keyboard site the plan did not anticipate, STOP and report — do not improvise a fix outside the spec.

```bash
git add specs/issue-386-focus-flake/LEDGER.md specs/issue-386-focus-flake/spec.md
git commit -m "docs(spec): #386 — record keyboard-site sweep in Errata"
```

### Task 3: Stability bar

**Files:**

- Modify: `specs/issue-386-focus-flake/spec.md` (Errata section only)

**Interfaces:**

- Consumes: Task 1's committed test change. Produces: recorded evidence that the fix holds under load.

- [ ] **Step 1: Loaded loop — 30 runs (or the count the owner sets at plan approval)**

Run the whole step as one background script so a mid-loop timeout cannot orphan the spinners. The `trap` guarantees cleanup even if the script dies; the pre-check before Step 2 verifies it:

```bash
cat > /tmp/ct386-loaded-loop.sh <<'EOF'
#!/usr/bin/env bash
cd /home/kv-thinkpad-t420-ubuntu/code/github/equationalapplications/curated-thoughts || exit 9
RUNS=${RUNS:-30}
N=$(nproc); SPIN=""
trap 'kill $SPIN 2>/dev/null' EXIT INT TERM
for i in $(seq 1 $((N+2))); do (while :; do :; done) & SPIN="$SPIN $!"; done
echo $SPIN > /tmp/ct386-spin.pids
LOADED=0
for i in $(seq 1 $RUNS); do
  pnpm vitest run src/__tests__/UnverifiedPromptModal.test.tsx 2>&1 | grep -q 'Tests  9 passed' && LOADED=$((LOADED+1))
done
kill $SPIN 2>/dev/null
echo "loaded green: $LOADED/$RUNS"
EOF
RUNS=30 bash /tmp/ct386-loaded-loop.sh
```

`RUNS` is the owner's plan-approval lever: if the owner sets a count other than 30, run `RUNS=<count> bash /tmp/ct386-loaded-loop.sh` and amend the spec's Test plan item 1 to the same number in the same commit (the spec outranks the plan). Note a bash `trap` does not run on SIGKILL, which is exactly why Step 2's pre-check exists.

Run the script with a generous timeout (600 s or more) or in the background — 30 loaded runs at several seconds each can exceed a 120 s tool timeout.

Expected: `loaded green: 30/30`. Judge each iteration by the `Tests  9 passed` line; a failing run prints `N failed | M passed` and cannot match, so counting errs toward failure, never false success.

- [ ] **Step 2: Unloaded loop — 20 runs**

First verify no spinner survived. The spinners are forked subshells of the script, so `ps` shows them as `bash /tmp/ct386-loaded-loop.sh` — do not grep for `while :`. Both checks must print nothing:

```bash
ps -o pid=,cmd= -p "$(tr ' ' ',' < /tmp/ct386-spin.pids | sed 's/^,//')" 2>/dev/null
pgrep -af ct386-loaded-loop.sh
```

If anything survives, kill those PIDs and note it in the Errata — leftover load would invalidate the unloaded runs.

```bash
UNLOADED=$(for i in $(seq 1 20); do pnpm vitest run src/__tests__/UnverifiedPromptModal.test.tsx 2>&1 | grep -c 'Tests  9 passed'; done | grep -c 1)
echo "unloaded green: $UNLOADED/20"
```

Expected: `unloaded green: 20/20`.

- [ ] **Step 3: Full frontend suite**

Run: `pnpm vitest run 2>&1 | tail -6`
Expected: every test file passes and the totals line shows 0 failed. Read the real totals; do not assume.

- [ ] **Step 4: Record results in the spec Errata and commit**

Fill `specs/issue-386-focus-flake/spec.md`'s Errata section with:

- The loaded count over 30 (or the owner's count), the unloaded count over 20, the full-suite totals, the machine profile (4 cores, 6 spinner processes), and the date.
- The pre-fix baseline for comparison: the identical file, pre-fix, failed 2 of 30 loaded runs (~7 percent).
- The honesty statement, verbatim intent: the loaded loop backs up the reasoning that each `waitFor` can only pass after the trap's effect ran — it does not prove the fix alone (a pass rate this high leaves residual chance of an unfixed flake surviving the bar).

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
