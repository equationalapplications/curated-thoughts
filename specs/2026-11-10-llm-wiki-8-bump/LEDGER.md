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
