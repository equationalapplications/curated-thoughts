# LEDGER — 2026-11-10-llm-wiki-8-bump

- 2026-11-10 — Investigation complete: diffed 7.13.0 vs 8.0.2 tarballs
  (DDL byte-identical; only default flips `librarian` legacy→ops,
  `maintenance` auto→deferred; react 8.0.2 `export *` from core; both
  schema-manifest exports present; `core-llm-tools` removal not consumed
  by CT). core-okf override found lockstep-lagged at 7.9.0 since #252.
  Age gate confirmed as dependabot-only (`.github/dependabot.yml:21`);
  internal family exempt per controller ruling 2026-11-10.
- 2026-11-10 — spec.md + plan.md written; Status Draft. Implementation
  begins.
- 2026-11-10 — Pins applied: package.json (4 deps + 2 overrides → 8.0.2;
  core-okf un-lagged from 7.9.0), schema_guard pin → 8.0.2,
  `makeWikiOptions` behavior pins (`librarian: legacy`, `maintenance:
  auto`) with rationale comment; 4 host-contract comment stamps reworded
  to "introduced 7.13.0, reverified 8.0.2". Lockfile moves only the five
  packages + core-okf (29-line diff).
- 2026-11-10 — Added guard test `makeWikiOptions engine-default pins` in
  wiki.test.ts; RED verified via pathspec stash (fails without the pin),
  GREEN with it.
- 2026-11-10 — Frontend gates: typecheck PASS; lint shows only the 10
  documented `.cargo-shared-target` false positives (0 in tracked
  source); vitest 609 passed / 10 failed — failure set byte-identical to
  clean main @ 7.13.0 (verified via stash + reinstall cycle), i.e.
  pre-existing local-env failures (node 26 family, #208), zero bump
  regressions. CI (node 22) authoritative.
- 2026-11-10 — Stamp sweep: no `7.13.0` outside intentional historical
  comment references. Rust gates (CI-mirroring chain: clippy ×2, tests ×2
  serial, engine_source_ref_gate `--ignored`) launched in background.
