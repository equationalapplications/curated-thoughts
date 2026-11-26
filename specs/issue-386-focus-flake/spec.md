# Spec: #386 — Stabilize the UnverifiedPromptModal focus test

Status: Draft (2026-11-26)
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
opens through these steps:

1. The component calls `useFocusTrap(dialogRef, ...)` at
   `src/components/queue/UnverifiedPromptModal.tsx:95`.
2. `useFocusTrap` registers its keydown handler and focuses the first
   button inside a passive `useEffect` at
   `src/a11y/useFocusTrap.ts:88-94`.
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

The product behavior is correct. A real browser runs passive effects
before paint, so the dialog always receives focus and the handler is
always registered before a user can press Tab. The defect is a
test-side race only.

## Proposed change

Each racy test must wait for the trap's passive effect before it
asserts. Both fixes use `waitFor`, which polls until the condition
holds, within its timeout. If the trap never runs, `waitFor` times
out and the test fails. The fixes keep the assertions' strength and
remove the races.

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

## Files touched

- `src/__tests__/UnverifiedPromptModal.test.tsx` — add the two
  `waitFor` wraps. No other change to the file.

No product code changes.

## Test plan

1. Run the fixed file 30 times under the same load profile (6 spinner
   processes on 4 cores). All 30 runs must pass.
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

- The other assertions in this file. They already use `waitFor` or
  run synchronously.
- `useFocusTrap` and the modal component.

## Errata

(to be filled by the implementation with the stability-bar results)
