# Tiptap `mergeAttributes()` Remediation (GHSA-cp6q-959q-f8rh) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pin the entire 12-package `@tiptap/*` suite to 3.30.6 via pnpm overrides, closing Dependabot alert #50 (and an unadvised DoS fixed in 3.30.5), and lock the security property in place with a real regression test.

**Architecture:** CT has no direct tiptap dependency — the suite arrives transitively through `@blocknote/*@0.54.0`, which declares `^3.29.2`. Remediation is therefore a `pnpm.overrides` block pinning all 12 packages in lockstep, plus matching `minimumReleaseAgeExclude` entries because 3.30.6 is younger than the repo's 14-day supply-chain gate. No application source changes. The one piece of new code is a regression test that asserts the `mergeAttributes()` prototype-manipulation fix is actually present in the resolved package — so a future override removal or downgrade fails loudly instead of silently reopening the hole.

**Tech Stack:** pnpm 10.33.2 (`packageManager` field), vitest 4.1.11, TypeScript, `@tiptap/core`, `@blocknote/{core,mantine,react}@0.54.0`

**Spec:** `docs/superpowers/specs/2026-09-02-tiptap-mergeattributes-dependabot-design.md`

## Global Constraints

- **Target version is exactly `3.30.6`** for all 12 packages. Not 3.30.4 (superseded 99 minutes later by a second security fix), not 3.31.0 (minor; changes `selected` semantics in `@tiptap/react` node views). Spec §2.1.
- **All 12 packages move together.** Mixed-version resolution across the tiptap suite is unsupported upstream: `@tiptap/core`, `extension-bold`, `extension-bubble-menu`, `extension-code`, `extension-floating-menu`, `extension-italic`, `extension-strike`, `extension-text`, `extension-underline`, `extensions`, `pm`, `react`.
- **No application source changes.** This is a dependency pin. The only new file is the regression test in Task 1.
- **`--frozen-lockfile` does not evaluate `minimumReleaseAge`.** Only `pnpm install --lockfile-only` (or a plain `pnpm install` that resolves) exercises the release-age gate. CI uses `--frozen-lockfile` in all five workflow steps, so CI *cannot* validate the exclude-list edits. Local verification is the only gate.
- **Branch:** `fix/tiptap-mergeattributes-3.30.6`, cut from `main`.
- **Commit trailers** (every commit in this plan):
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01X5FgsxGFNXes4GBioSch1W
  ```

## Environment Warning — read before Task 1

The working tree's `node_modules/@tiptap/core` is currently **3.22.5**, while `pnpm-lock.yaml` says **3.30.2**. The local install is stale and does not match the lockfile. Every verification step in this plan that runs code (the regression test especially) reads `node_modules`, not the lockfile — so a stale install will produce meaningless results. Task 1 Step 1 resynchronises before anything else. Do not skip it.

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `src/__tests__/tiptapMergeAttributes.test.ts` | **Create.** Regression test asserting `mergeAttributes()` does not let an own `__proto__` key reach `for...in` enumeration. Pins the security property independent of the version string. | 1 |
| `package.json` (lines 7–16, `pnpm.overrides`) | **Modify.** Add the 12 exact-version pins plus a removal-condition comment. | 2 |
| `pnpm-workspace.yaml` (lines 26–38) | **Modify.** Swap the 12 `@tiptap/*@3.30.2` exclude entries and the stale section comment to 3.30.6. | 2 |
| `pnpm-lock.yaml` | **Regenerate.** Mechanical output of `pnpm install --lockfile-only`. Never hand-edit. | 2 |

---

### Task 1: Regression test for the `mergeAttributes()` security property

Write the test **first, against the vulnerable version**, so we see it fail for the right reason. This is the only automated check in the entire suite that touches real tiptap — see the note at the end of this task.

**Files:**
- Create: `src/__tests__/tiptapMergeAttributes.test.ts`

**Interfaces:**
- Consumes: `mergeAttributes` from `@tiptap/core` (resolvable from the repo root; verified).
- Produces: nothing later tasks import. Task 2 flips this test from RED to GREEN.

- [ ] **Step 1: Resynchronise `node_modules` with the lockfile**

The tree is stale (3.22.5 installed vs 3.30.2 locked). Fix that before measuring anything:

```bash
pnpm install --frozen-lockfile
node -p "require('./node_modules/@tiptap/core/package.json').version"
```

Expected: prints `3.30.2`. If it prints anything else, stop and investigate — every later step depends on `node_modules` matching the lockfile.

- [ ] **Step 2: Write the failing test**

Create `src/__tests__/tiptapMergeAttributes.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { mergeAttributes } from "@tiptap/core";

/**
 * Regression test for GHSA-cp6q-959q-f8rh (Dependabot alert #50).
 *
 * `mergeAttributes()` assigned keys from `Object.entries()` with plain bracket
 * assignment, so an own `__proto__` key in attacker-controlled JSON replaced the
 * merged object's prototype instead of becoming a normal property. Own-property
 * checks (`Object.keys`) then showed nothing, but ProseMirror's
 * `DOMSerializer.renderSpec()` enumerates attribute objects with `for...in`,
 * which walks the prototype chain — so the injected values reached
 * `setAttribute()` on the rendered element.
 *
 * This test asserts the security property directly rather than asserting a
 * version number, so it keeps failing if the pnpm override in package.json is
 * ever removed or rolled back to a vulnerable release.
 *
 * Fixed in @tiptap/core 3.30.4; this repo pins the whole suite at 3.30.6.
 */
describe("mergeAttributes prototype manipulation (GHSA-cp6q-959q-f8rh)", () => {
  // JSON.parse is required: an object literal would treat __proto__ as a
  // setter, not as the own property an attacker actually delivers over the wire.
  const hostileAttributes = () =>
    JSON.parse('{"__proto__":{"src":"https://evil.test/x.png","onerror":"alert(1)"}}');

  it("does not expose injected values through for...in enumeration", () => {
    const merged = mergeAttributes(hostileAttributes());

    // This is the exact enumeration DOMSerializer.renderSpec() performs.
    const enumerated: string[] = [];
    for (const key in merged) {
      enumerated.push(key);
    }

    expect(enumerated).not.toContain("src");
    expect(enumerated).not.toContain("onerror");
  });

  it("does not resolve injected values through the prototype chain", () => {
    const merged = mergeAttributes(hostileAttributes()) as Record<string, unknown>;

    expect(merged.src).toBeUndefined();
    expect(merged.onerror).toBeUndefined();
  });

  it("leaves Object.prototype untouched", () => {
    mergeAttributes(hostileAttributes());

    expect(({} as Record<string, unknown>).src).toBeUndefined();
  });

  it("still merges ordinary attributes", () => {
    const merged = mergeAttributes({ class: "a" }, { "data-x": "1" });

    expect(merged).toMatchObject({ class: "a", "data-x": "1" });
  });
});
```

- [ ] **Step 3: Run the test and confirm it fails for the right reason**

```bash
pnpm vitest run src/__tests__/tiptapMergeAttributes.test.ts
```

Expected on 3.30.2: the first two tests **FAIL**, the last two **PASS**.

The failures must read as an `expect(...).not.toContain("src")` mismatch (enumerated is `[ 'src', 'onerror' ]`) and `merged.src` being `"https://evil.test/x.png"` rather than `undefined`. That specific output is the vulnerability reproducing — this exact behaviour was confirmed against the installed package while writing this plan.

If instead the file fails to import `@tiptap/core`, or all four tests pass, stop: either resolution is not what the lockfile says (re-check Step 1) or the suite is not on a vulnerable version. Do not proceed to Task 2 without a genuine RED.

- [ ] **Step 4: Commit the failing test**

Committing RED is deliberate here — it puts the vulnerability's reproduction in the history, so the next commit demonstrably closes it.

```bash
git add src/__tests__/tiptapMergeAttributes.test.ts
git commit -m "test(security): reproduce tiptap mergeAttributes prototype manipulation

Failing regression test for GHSA-cp6q-959q-f8rh (Dependabot #50). Asserts the
security property rather than a version string, so it also guards against the
pnpm override being removed later. Goes green with the 3.30.6 pin.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01X5FgsxGFNXes4GBioSch1W"
```

> **Why this test carries the whole verification burden:** every other
> BlockNote-touching test in the suite (`EditorPane.test.tsx:77-81`,
> `EntitySummarySection.test.tsx:19-29`) calls `vi.mock("@blocknote/react")`
> and `vi.mock("@blocknote/mantine")`, so `pnpm test` never loads real tiptap.
> The spec's claim that the editor suite "exercises the BlockNote editor on the
> new tiptap" does not hold. After this task it is true of exactly one file —
> this one — and the manual smoke in Task 3 remains mandatory rather than
> optional.

---

### Task 2: Pin the suite to 3.30.6

**Files:**
- Modify: `package.json:7-16` (`pnpm.overrides`)
- Modify: `pnpm-workspace.yaml:26-38` (section comment + 12 exclude entries)
- Regenerate: `pnpm-lock.yaml`

**Interfaces:**
- Consumes: the failing test from Task 1.
- Produces: a resolved tree at 3.30.6 that Task 3 verifies.

- [ ] **Step 1: Add the overrides to `package.json`**

Replace lines 7–16 (the `"overrides"` object) with:

```json
    "overrides": {
      "undici@6": "6.28.0",
      "undici@7": "7.29.0",
      "js-yaml": "4.3.1",
      "postcss": "8.5.26",
      "brace-expansion@1": "1.1.18",
      "brace-expansion@5": "5.0.9",
      "@equationalapplications/core-llm-wiki": "6.2.0",
      "@equationalapplications/core-okf": "6.1.0",
      "_comment_tiptap": "SECURITY PIN — GHSA-cp6q-959q-f8rh (Dependabot #50) + the Markdown-attribute DoS fixed in 3.30.5. BlockNote 0.54.0 declares ^3.29.2 and resolves to a vulnerable version on its own. These are hard pins and do NOT float forward: remove the whole @tiptap block once BlockNote's own range resolves to >= 3.30.6, then drop the matching pnpm-workspace.yaml exclude entries.",
      "@tiptap/core": "3.30.6",
      "@tiptap/extension-bold": "3.30.6",
      "@tiptap/extension-bubble-menu": "3.30.6",
      "@tiptap/extension-code": "3.30.6",
      "@tiptap/extension-floating-menu": "3.30.6",
      "@tiptap/extension-italic": "3.30.6",
      "@tiptap/extension-strike": "3.30.6",
      "@tiptap/extension-text": "3.30.6",
      "@tiptap/extension-underline": "3.30.6",
      "@tiptap/extensions": "3.30.6",
      "@tiptap/pm": "3.30.6",
      "@tiptap/react": "3.30.6"
    }
```

JSON has no comment syntax, so the rationale rides as a `_comment_tiptap` key. pnpm ignores override keys that match no package name. If a reviewer objects to the pseudo-key, move the text into the spec and delete the line — the pins are what matter.

- [ ] **Step 2: Update `pnpm-workspace.yaml`**

Replace lines 26–38 — both the section comment and all 12 entries:

```yaml
  # --- @blocknote/react@0.54.0 editor stack (all @tiptap at 3.30.6) ---
  # NOTE: unlike the grandfathered entries above, this block DOES change resolved
  # versions — it is the GHSA-cp6q-959q-f8rh security pin (see package.json
  # pnpm.overrides). 3.30.6 published 2026-08-31; clears the 14-day window
  # 2026-09-14, after which these entries can be dropped.
  - '@tiptap/core@3.30.6'
  - '@tiptap/extension-bold@3.30.6'
  - '@tiptap/extension-bubble-menu@3.30.6'
  - '@tiptap/extension-code@3.30.6'
  - '@tiptap/extension-floating-menu@3.30.6'
  - '@tiptap/extension-italic@3.30.6'
  - '@tiptap/extension-strike@3.30.6'
  - '@tiptap/extension-text@3.30.6'
  - '@tiptap/extension-underline@3.30.6'
  - '@tiptap/extensions@3.30.6'
  - '@tiptap/pm@3.30.6'
  - '@tiptap/react@3.30.6'
```

The added NOTE matters: the file's header comment (lines 4–8) claims "this list changes NO resolved version," which is true of the `37c7db9` entries but false of this block. Leaving that unqualified would mislead the next reader.

- [ ] **Step 3: Regenerate the lockfile — this is the release-age gate**

```bash
pnpm install --lockfile-only
```

Expected: succeeds. This is the **only** command that evaluates `minimumReleaseAge`; if the Step 2 exclude entries are wrong, incomplete, or misspelled, this is where it fails, with an error naming the package whose release age was refused. CI will never catch a mistake here.

- [ ] **Step 4: Install the regenerated tree**

`--lockfile-only` does not touch `node_modules`. The test needs the real files:

```bash
pnpm install
node -p "require('./node_modules/@tiptap/core/package.json').version"
```

Expected: prints `3.30.6`.

- [ ] **Step 5: Verify resolution — acceptance criteria 1 and 2**

```bash
# No package left behind (plain or peer-suffixed form)
grep -c '@tiptap/.*@3\.30\.2' pnpm-lock.yaml            # → 0

# All 12 moved, rather than some having vanished
grep -oE "@tiptap/[a-z-]+@3\.30\.6" pnpm-lock.yaml | sort -u | wc -l   # → 12

# No stale version reference anywhere in the workspace file
grep -c '3\.30\.2' pnpm-workspace.yaml                   # → 0
```

All three must match. `grep -c` exits non-zero when it counts 0, so run them individually rather than chaining with `&&`.

- [ ] **Step 6: Run the Task 1 test — it must now pass**

```bash
pnpm vitest run src/__tests__/tiptapMergeAttributes.test.ts
```

Expected: **all four tests PASS**. This is the moment the vulnerability is demonstrably closed — the same assertions that failed in Task 1 Step 3 now hold.

- [ ] **Step 7: Commit**

```bash
git add package.json pnpm-workspace.yaml pnpm-lock.yaml
git commit -m "fix(deps): bump @tiptap/* 3.30.2 -> 3.30.6 (GHSA-cp6q-959q-f8rh, Dependabot #50)

Pins all 12 @tiptap packages via pnpm.overrides. BlockNote 0.54.0 declares
^3.29.2 and no BlockNote release ships a patched tiptap yet, so an override is
the only route.

Targets 3.30.6 rather than the advisory's first_patched_version of 3.30.4:
3.30.4 was superseded 99 minutes after publication by 3.30.5, which fixes a
second security issue (Markdown-attribute DoS) that has no Dependabot advisory
and would never raise an alert. 3.31.0 is held back — it changes 'selected'
semantics in @tiptap/react node views, which BlockNote 0.54.0 has not been
tested against.

Turns the Task 1 regression test green.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01X5FgsxGFNXes4GBioSch1W"
```

---

### Task 3: Verify the app still works

**Files:** none modified unless a gate fails.

**Interfaces:**
- Consumes: the pinned tree from Task 2.
- Produces: evidence for the PR body.

- [ ] **Step 1: Lockfile consistency — acceptance criterion 4**

```bash
pnpm install --frozen-lockfile
```

Expected: succeeds. Distinct from Task 2 Step 3: this performs no resolution and therefore re-checks nothing about release age. It proves only that the committed lockfile matches the committed manifest — which is exactly what CI will run.

- [ ] **Step 2: Typecheck — acceptance criterion 5**

```bash
pnpm typecheck
```

Expected: passes with no errors. This is the one automated gate that genuinely exercises the upgrade across the whole suite, because BlockNote's types are structurally checked against the pinned tiptap types. A mixed-version pin would most likely surface here.

- [ ] **Step 3: Full test suite — acceptance criterion 6**

```bash
pnpm test
```

Expected: passes, including the new `tiptapMergeAttributes.test.ts`.

Interpret this honestly in the PR body: apart from the new file, the suite mocks `@blocknote/react` and `@blocknote/mantine`, so a green run is **not** evidence that the editor works on 3.30.6. It is evidence of no regression elsewhere. Step 5 is what actually tests the editor.

- [ ] **Step 4: Lint — acceptance criterion 7**

```bash
pnpm lint
```

Expected: passes. If ESLint objects to the `_comment_tiptap` key or the new test file, fix the lint error rather than deleting the assertion.

- [ ] **Step 5: Manual editor smoke — acceptance criterion 9, mandatory**

```bash
pnpm tauri dev
```

Given Step 3's mocking, this is the only real exercise of the editor. Walk it:

1. Open a wiki entry with an existing body; confirm content renders (no blank pane, no console errors).
2. Type a paragraph; apply **bold**, *italic*, and `code` via the toolbar and via keyboard shortcuts.
3. Build a **nested bulleted list** — at least two levels deep, with text at both levels.
4. Save, navigate away, reopen the entry.
5. Confirm the nested list still has its hierarchy, and the marks survived.

Step 3–5 are called out specifically because 3.30.6 changed nested-list Markdown serialization ("Nested lists exported to Markdown now keep their hierarchy when the file is read back"). That is the single highest-risk behavioural change in the 3.30.2 → 3.30.6 range, and no automated test in this repo covers it.

Record the result — pass or fail — in the PR body. If it fails, stop and report rather than proceeding to Task 4; a serialization regression is a reason to reconsider the target version, not to ship.

- [ ] **Step 6: Commit only if a gate required a fix**

If Steps 1–5 all passed with no edits, there is nothing to commit — proceed to Task 4. If a fix was needed:

```bash
git add -A
git commit -m "fix: address <specific gate failure> from tiptap 3.30.6 bump

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01X5FgsxGFNXes4GBioSch1W"
```

---

### Task 4: Ship and confirm remediation

**Files:** none.

**Interfaces:**
- Consumes: verified branch from Task 3.
- Produces: closed Dependabot alert #50.

- [ ] **Step 1: Push and open the PR**

```bash
git push -u origin fix/tiptap-mergeattributes-3.30.6
```

Then open the PR with a body covering: the 3.30.6-over-3.30.4 rationale (spec §2.1), the three resolution greps from Task 2 Step 5 with their actual output, the gate results from Task 3, and an explicit statement of the Task 3 Step 5 manual smoke result. Link the spec. End the body with:

```
🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_01X5FgsxGFNXes4GBioSch1W
```

- [ ] **Step 2: Merge when green**

House convention is a regular merge commit — **no squash**. Squashing would collapse the RED test commit into the fix, losing the demonstration that the test actually caught the vulnerability.

- [ ] **Step 3: Confirm the alert closed — acceptance criterion 8**

Dependabot re-scans the default branch after a manifest/lockfile change; allow up to a day.

```bash
# Positive assertion: #50 specifically reached "fixed"
gh api repos/equationalapplications/curated-thoughts/dependabot/alerts --paginate \
  --jq '.[] | select(.number == 50) | .state'          # → fixed

# Repo-wide: zero open alerts
gh api repos/equationalapplications/curated-thoughts/dependabot/alerts --paginate \
  --jq '.[] | select(.state == "open")'                # → empty
```

Both are required. The second alone is insufficient: a *dismissed* #50 would also satisfy it while proving nothing about remediation.

- [ ] **Step 4: File the two follow-up chores**

Neither belongs in this PR, and both are lost if not written down now:

1. **Drop the exclude entries after 2026-09-14** — the date 3.30.6 clears the 14-day `minimumReleaseAge` window. The `pnpm.overrides` pins stay; only the `pnpm-workspace.yaml` entries go.
2. **CI never validates `minimumReleaseAgeExclude`.** All five install steps (`ci.yml:60`, `ci.yml:113`, `ci.yml:146`, `build.yml:138`, `release.yml:70`) use `--frozen-lockfile`, which skips resolution. A broken exclude list passes CI and only breaks for whoever next regenerates the lockfile. Consider a scheduled job running `pnpm install --lockfile-only` and failing on a dirty lockfile.

Also worth a line in the issue tracker: the override is a hard pin. Once BlockNote publishes a release whose own `^` range lands on ≥ 3.30.6, remove the override block entirely. `@blocknote/core` was still 0.54.0 as of 2026-09-02.

---

## Self-Review

**Spec coverage.** §3.1 → Task 2 Step 1. §3.2 → Task 2 Step 2. §3.3 → Task 2 Steps 3–4. §4 criteria 1–2 → Task 2 Step 5; 3 → Task 2 Step 3; 4 → Task 3 Step 1; 5 → Step 2; 6 → Step 3; 7 → Step 4; 8 → Task 4 Step 3; 9 → Task 3 Step 5. §5 removal reminder → Task 2 Step 1 comment and Task 4 Step 4. §6 plan → Tasks 2–4.

**One deliberate addition beyond the spec:** Task 1's regression test. The spec assumed `pnpm test` would exercise the upgraded editor; inspection of `EditorPane.test.tsx:77-81` and `EntitySummarySection.test.tsx:19-29` shows every BlockNote test mocks the library, so nothing in the suite loaded real tiptap. Without Task 1 the pin would have had no automated verification at all, and its later removal would be silent. The spec should be amended to match — its criterion 6 rationale is currently inaccurate.

**Two environment facts the spec did not know**, both surfaced while writing this plan and both capable of invalidating local verification: `node_modules` holds 3.22.5 against a lockfile saying 3.30.2 (Task 1 Step 1 resynchronises), and `--lockfile-only` leaves `node_modules` untouched so a plain `pnpm install` is required before the test can be believed (Task 2 Step 4).

**Placeholders:** none. Every code and command step carries literal content; the only conditional step (Task 3 Step 6) states its condition and its skip path.

**Type consistency:** the sole new symbol is the import of `mergeAttributes` from `@tiptap/core`, verified resolvable from the repo root and confirmed to be a function. No task references a symbol another task did not define.
