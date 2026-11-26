# Spec: #386 — Stabilize the UnverifiedPromptModal focus test

Status: Implemented (2026-11-26 — PR #387; stability bar and T4 verdict recorded in Errata and review records)
Issue: #386
Branch: fix/386-focus-flake

## Problem

The continuous integration (CI) frontend job failed once on the test
"focus moves into the dialog on open (first action button focused)".
The failure happened on pull request (PR) #384 (run 45805049376, job
139857955170, 2026-11-26). The rerun of the identical tree passed. The
commit under test was docs-only. The test code was identical in all
three runs. The test is flaky under CI load. A flaky test costs a
rerun and erodes trust in the frontend job. A local load loop also
caught the sibling timing test in this file (see Root cause).

## Root cause

The race sits between the test and one asynchronous step. The modal
opens through these steps (test-file line numbers refer to the
pre-fix tree `53baa084`):

1. The component calls `useFocusTrap(dialogRef, ...)` at
   `src/components/queue/UnverifiedPromptModal.tsx:95`.
2. `useFocusTrap` registers its keydown handler and takes initial
   focus inside a passive `useEffect` at
   `src/a11y/useFocusTrap.ts:88-129` (registration at :123).
3. React schedules passive effects after the commit. Under load, the
   scheduler runs them late.
4. `findByText` resolves as soon as the modal text appears. The tests
   then interact with the dialog before the passive effect has run.
5. The test "focus moves into the dialog on open" asserts
   `document.activeElement` immediately, with no `waitFor`
   (`src/__tests__/UnverifiedPromptModal.test.tsx:136-142`). When the
   poll wins the race, focus is still on `body`, and the assertion
   fails. This test failed once in CI (see Problem).
6. The test "Tab from the last focusable wraps to the first focusable"
   has the same race one step later
   (`src/__tests__/UnverifiedPromptModal.test.tsx:183-196`). It calls
   `last.focus()` (a direct call, so it works), then fires the Tab
   event. If the trap effect has not run, the keydown handler is not
   registered, focus stays on "Not now", and the assertion at line 193
   fails.

Evidence from real runs (2026-11-26, this machine):

- Unloaded loop: 20 sequential runs of the file, 20 passed
  (~1.6 s per run).
- Loaded loop (6 spinner processes on 4 cores): 30 sequential runs,
  28 passed, 2 failed (~7 percent failure rate).
- Detail loop under the same load: a captured failure of the Tab test
  at line 193. The received element was "Not now"; the expected
  element was "Confirm". The modal text was fully rendered, so the
  dialog had mounted, but the trap had not taken focus.

The product behavior is correct. The trap effect lands within about
one scheduler tick of the dialog mounting — a window a human cannot
Tab into, unlike the test's poll, which resolves on that same tick.
The defect is a test-side race only.

## Proposed change

Each racy test must wait for the trap's passive effect before it
asserts. All three fixes use `waitFor`, which polls until the
condition holds, within its timeout. If the trap never runs, `waitFor`
times out and the test fails. The fixes keep the assertions' strength
and remove the races.

Fix 1 — test "focus moves into the dialog on open", lines 139-141 of
`src/__tests__/UnverifiedPromptModal.test.tsx`. Wrap the assertion in
`waitFor`:

```tsx
await waitFor(() => {
  expect(document.activeElement).toBe(
    screen.getByRole("button", { name: /^confirm$/i }),
  );
});
```

Fix 2 — test "Tab from the last focusable wraps to the first
focusable", after `await screen.findByText(...)` and before the
`last.focus()` call. Wait for the trap's initial focus to land first:

```tsx
await waitFor(() => {
  expect(document.activeElement).toBe(
    screen.getByRole("button", { name: /^confirm$/i }),
  );
});
```

The wait proves the trap is active, so its keydown handler is
registered. The rest of the test stays unchanged.

Fix 3 — test "on close, focus returns to the previously focused
element". This fix is DEFENSIVE, not a reproduced failure. No run of
this test has failed. React runs the trap's cleanup inside the
`act()`-wrapped `fireEvent.click`, so the restore normally happens
before the click returns. The wrap is insurance for the same
mechanism family: the trap's effect and its cleanup are passive
flushes, and the current `waitFor` covers only the dialog's removal.
Wrap the assertion at line 216 in `waitFor` too:

```tsx
await waitFor(() => {
  expect(document.activeElement).toBe(opener);
});
```

`waitFor` passes at once when the restore already happened, so the
wrap cannot mask a missing restore; a restore delayed under 1 s
(waitFor's default timeout) would pass. The owner's spec approval
carried a sanity check for leftover races in this file; the T3 review
ruled this wrap defensive-only, and the owner kept it at plan approval
(owner ruling, 2026-11-26).

## Files touched

- `src/__tests__/UnverifiedPromptModal.test.tsx` — add the three
  `waitFor` wraps. No other change to the file.

No product code changes.

## Test plan

1. Run the fixed file 100 times under the same load profile (6 spinner
   processes on 4 cores; count raised from 30 by owner plan ruling,
   2026-11-26). All 100 runs must pass.
2. Run the fixed file 20 times unloaded. All 20 runs must pass.
3. Run the full frontend suite once (`pnpm vitest run`). All tests
   must pass. The implementer must read the real output.
4. Record the three results in the Errata section of this spec. The
   Errata section is the durable home; the plan and ledger files get
   deleted before merge.

## Rejected alternatives

- We rejected a `flushMicrotasks` call before the assertion: the race
  spans a scheduler tick, not one microtask, so one flush does not
  close it.
- We rejected moving the focus into a layout effect: that changes
  product timing to fix a test-side race. The review rounds on PR
  #379 chose the passive-effect design; we do not reopen it.
- We rejected deleting the assertion: the dialog-focus behavior is an
  accessibility requirement from the R3b review round. The test must
  keep proving it.
- We rejected `vi.waitFor` with a short custom timeout: the default
  `waitFor` timeout already covers loaded runners, and a second timer
  API adds noise.

## Out of scope

- The click-driven tests in this file. Clicks reach React props set at
  commit time, so they do not race the trap.
- `useFocusTrap` and the modal component.

## Errata

- Keyboard-site sweep (owner sanity check, 2026-11-26, post-fix tree `1771faf2`): exactly one `fireEvent.keyDown` site in the file (the Tab test, now behind the Fix 2 `waitFor`); all other interactions are `fireEvent.click`, which reach React props set at commit time and do not race the trap. No Shift+Tab or Escape keydown tests exist.
- Stability bar (2026-11-26, post-fix tree `1771faf2`, machine: 4 cores, 6 spinner processes for the loaded leg):
  - Loaded loop: 100/100 runs green.
  - Unloaded loop: 20/20 runs green (after a verified no-surviving-spinner pre-check).
  - Full frontend suite: 631 passed, 1 skipped (632 total), 0 failed, ~70 s.
  - Pre-fix baseline for comparison: the identical file, pre-fix, failed 2 of 30 loaded runs (~7 percent point estimate), and failed once in CI on PR #384 (the focus-on-open test). The 2/30 failures were captured as summary lines only, so they are not attributable to a specific test; the only locally captured named failure was the Tab test (detail loop).
  - Honesty statement: the loaded loop backs up the reasoning that each `waitFor` can only pass after the trap's effect ran — it does not prove the fix alone. On the 2-in-30 point estimate, an unfixed flake surviving 100 green runs sits around 0.1 percent, but the small-sample spread of that base rate is wide, so the mechanism argument, not the loop, is the primary evidence.
  - Counting method and pipeline note: the loaded leg counted with `grep -q 'Tests  9 passed'` (that shell context emitted no color codes, so the literal pattern matched). The unloaded leg first ran the same literal pattern and undercounted 0/20 against visibly passing runs — vitest emitted ANSI codes there (`Tests \x1b[22m ...`); it was rerun with the ANSI-safe `grep -cE 'Tests.*9 passed'`, which requires the literal "9 passed" and cannot match a "1 failed | 8 passed" summary line. Both patterns fail toward undercounting, never false-successes. Each leg's raw output was read.
