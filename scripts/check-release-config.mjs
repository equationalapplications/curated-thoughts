#!/usr/bin/env node
/**
 * Release-config smoke test.
 *
 * `release.yml` runs on `workflow_run` AFTER a merge to main, so no PR check
 * exercises the release pipeline. A broken `.releaserc.json` or an
 * incompatible changelog-preset bump therefore reaches main unnoticed.
 * The preset/writer incompatibility this script catches throws inside
 * `generateNotes` — semantic-release v25 invokes that phase before
 * `prepare`, where `@semantic-release/git` would commit `CHANGELOG.md`
 * and the version files — so the release fails before any side effects
 * begin.
 *
 * This script closes that gap. It loads the REAL `.releaserc.json` (never a
 * fixture — a fixture drifts from the config it is meant to protect) and
 * drives the two PURE semantic-release plugins over synthetic commits:
 *
 *   - @semantic-release/commit-analyzer      (config -> release type)
 *   - @semantic-release/release-notes-generator  (commits -> markdown)
 *
 * Both are pure functions of (config, commits): no network, no GitHub token,
 * no git writes. The side-effecting plugins (changelog, npm, exec, git,
 * github) are deliberately NOT exercised — covering them would require a
 * full `semantic-release --dry-run`, which needs credentials and fails
 * ERELEASEBRANCHES on PR head refs.
 *
 * Run: node scripts/check-release-config.mjs
 * Exits 0 when every check passes, 1 otherwise.
 */
import { analyzeCommits } from '@semantic-release/commit-analyzer';
import { generateNotes } from '@semantic-release/release-notes-generator';
import { readFileSync } from 'node:fs';

const REPO_URL = 'https://github.com/equationalapplications/curated-thoughts';

const rc = JSON.parse(readFileSync('.releaserc.json', 'utf8'));

/** Pull one plugin's options out of the real .releaserc.json. */
function pluginConfig(name) {
  const entry = rc.plugins.find((p) => Array.isArray(p) && p[0] === name);
  if (!entry) {
    throw new Error(`.releaserc.json has no configured plugin "${name}"`);
  }
  return entry[1] ?? {};
}

const silentLogger = { log: () => {}, warn: () => {}, error: () => {} };

function commit(message) {
  return {
    hash: 'a1b2c3d4e5f60718293a4b5c6d7e8f9012345678',
    subject: message.split('\n')[0],
    message,
  };
}

function context(commits) {
  return {
    commits,
    logger: silentLogger,
    cwd: process.cwd(),
    options: { repositoryUrl: REPO_URL },
    lastRelease: { version: '1.0.0', gitTag: 'v1.0.0', gitHead: 'f'.repeat(40) },
    nextRelease: {
      version: '1.1.0',
      gitTag: 'v1.1.0',
      type: 'minor',
      gitHead: 'a'.repeat(40),
      channel: null,
    },
  };
}

/**
 * Every rule in .releaserc.json's `releaseRules`, asserted end to end.
 * `null` means "no release" (the rule sets `release: false`).
 */
const VERSION_MATRIX = [
  // A breaking change outranks the type-based rules below. Configured rules
  // are consulted instead of the built-in defaults whenever any of them
  // matches, so without an explicit `breaking` rule a `feat!` commit matches
  // only `{ type: 'feat' }` and releases as minor (issue #160).
  //
  // Do NOT turn this into an ordering assertion: the analyzer takes the
  // highest release type among ALL matching rules, so the breaking rule works
  // from any position in the array.
  {
    name: 'feat! -> major',
    message: 'feat(api)!: drop legacy path\n\nBREAKING CHANGE: removed the v1 path.',
    expected: 'major',
  },
  {
    name: 'fix! -> major',
    message: 'fix(api)!: drop legacy path\n\nBREAKING CHANGE: removed the v1 path.',
    expected: 'major',
  },
  // Same behavior without the `!` marker — breaking changes can be
  // expressed either via the subject `!` or via a `BREAKING CHANGE:`
  // footer in the body. The `{ breaking: true }` rule matches both.
  {
    name: 'feat w/ footer -> major',
    message: 'feat(api): drop legacy path\n\nBREAKING CHANGE: removed the v1 path.',
    expected: 'major',
  },
  {
    name: 'fix w/ footer -> major',
    message: 'fix(api): drop legacy path\n\nBREAKING CHANGE: removed the v1 path.',
    expected: 'major',
  },
  { name: 'feat -> minor', message: 'feat(vault): add bootstrap', expected: 'minor' },
  { name: 'fix -> patch', message: 'fix(vault): correct allowlist gate', expected: 'patch' },
  // First-party dependency adoptions (issue #253): the `deps` scope is
  // human-first-party-only by convention — `.github/dependabot.yml` writes
  // bot bumps as `chore(bot)`. Exact `deps` scope match: `deps` never
  // generalizes to `deps*` (that would re-match `deps-dev`).
  {
    name: 'chore(deps) first-party adoption -> patch',
    message:
      'chore(deps): adopt @equationalapplications llm-wiki 7.9.0 (engine migration 13 schema sync) (#252)',
    expected: 'patch',
  },
  {
    name: 'chore(bot) dependabot bump -> no release',
    message: 'chore(bot): bump the minor-and-patch group with 11 updates',
    expected: null,
  },
  {
    name: 'chore(deps-dev) legacy bot subject -> no release',
    message: 'chore(deps-dev): bump vitest from 4.1.11 to 5.0.0',
    expected: null,
  },
  {
    name: 'plain chore -> no release',
    message: 'chore: tidy config',
    expected: null,
  },
  { name: 'perf -> no release', message: 'perf(embed): speed up embedding', expected: null },
  { name: 'style -> patch', message: 'style(vault): format imports', expected: 'patch' },
  { name: 'revert -> no release', message: 'revert: undo change', expected: null },
];

const NOTES_COMMITS = [
  commit('feat(vault): add bootstrap\n\nAdds lazy parent creation.'),
  commit('fix(vault): correct allowlist gate\n\nCloses #119'),
  commit('feat(api)!: drop legacy path\n\nBREAKING CHANGE: removed the v1 path.'),
];

const results = [];
function check(name, ok, detail = '') {
  results.push({ name, ok, detail });
}

for (const row of VERSION_MATRIX) {
  const actual = await analyzeCommits(
    pluginConfig('@semantic-release/commit-analyzer'),
    context([commit(row.message)]),
  );
  check(
    `version-bump: ${row.name}`,
    actual === row.expected,
    `expected ${String(row.expected)}, got ${String(actual)}`,
  );
}

let notes = '';
let notesError = null;
try {
  notes = await generateNotes(
    pluginConfig('@semantic-release/release-notes-generator'),
    context(NOTES_COMMITS),
  );
} catch (error) {
  notesError = error;
}

check(
  'release notes render without throwing',
  notesError === null,
  notesError ? notesError.message : '',
);

if (notesError === null) {
  check('notes are non-empty', typeof notes === 'string' && notes.trim().length > 0);
  check('feat subject rendered', notes.includes('add bootstrap'));
  check('fix subject rendered', notes.includes('correct allowlist gate'));
  check('breaking-change section rendered', /BREAKING/i.test(notes));
  // Match the URL only when it appears as a `/commit/` path inside a markdown
  // link — semantic-release formats commit refs as
  // `[short](https://.../commit/long)`. Anchoring on `(…/commit/` avoids
  // CodeQL's "incomplete URL substring sanitization" warning that a bare
  // `notes.includes(REPO_URL)` would trip.
  check(
    'commit links point at the repo',
    /\(https:\/\/github\.com\/equationalapplications\/curated-thoughts\/commit\//.test(notes),
  );
  check('no "[object Object]" leaked into notes', !notes.includes('[object Object]'));
  check('no bare "undefined" leaked into notes', !/\bundefined\b/.test(notes));
}

// Pure-adoption release (issue #253): a version cut whose only commits are a
// first-party `chore(deps)` adoption plus bot chores must still render its
// Dependencies section. The preset REPLACES its default types with
// presetConfig types, and `findTypeEntry` is first-match-wins, so the scoped
// `{ type: 'chore', scope: 'deps' }` entry MUST precede the unscoped hidden
// `chore` entry. A wrongly ordered list does NOT fail the non-empty check
// (the header always renders) — it fails the two checks below that look for
// the Dependencies section and the adoption subject.
const ADOPTION_COMMITS = [
  commit(
    'chore(deps): adopt @equationalapplications llm-wiki 7.9.0 (engine migration 13 schema sync) (#252)',
  ),
  commit('chore(bot): bump the minor-and-patch group with 11 updates'),
];

let adoptionNotes = '';
let adoptionError = null;
try {
  adoptionNotes = await generateNotes(
    pluginConfig('@semantic-release/release-notes-generator'),
    context(ADOPTION_COMMITS),
  );
} catch (error) {
  adoptionError = error;
}

check(
  'adoption-only notes render without throwing',
  adoptionError === null,
  adoptionError ? adoptionError.message : '',
);

if (adoptionError === null) {
  check(
    'adoption-only notes are non-empty',
    typeof adoptionNotes === 'string' && adoptionNotes.trim().length > 0,
  );
  check(
    'adoption notes have a Dependencies section',
    adoptionNotes.includes('Dependencies'),
  );
  check(
    'adoption subject rendered under the Dependencies heading',
    /### Dependencies[\s\S]*llm-wiki 7\.9\.0/.test(adoptionNotes),
  );
  check(
    'chore(bot) subject absent from notes',
    !adoptionNotes.includes('bump the minor-and-patch group'),
  );
}

let failed = 0;
for (const result of results) {
  if (result.ok) {
    console.log(`PASS  ${result.name}`);
  } else {
    failed += 1;
    console.log(`FAIL  ${result.name}${result.detail ? ` — ${result.detail}` : ''}`);
  }
}
console.log('');

if (failed > 0) {
  console.error(`release-config smoke test: ${failed} of ${results.length} checks FAILED`);
  process.exit(1);
}
console.log(`release-config smoke test: all ${results.length} checks passed`);
