# Plan: Strict card wording (#355)

Spec: specs/2026-11-26-strict-card-wording/spec.md (Reviewed — owner chat ruling 2026-11-26 = spec + plan approval)

## Commits

1. `fix(privacy): make Strict card copy honest about embeddings` — replace the Strict summary string in `src/components/privacy/PrivacyModeCards.tsx:12` with the spec's proposed copy.
2. `chore(spec): mark strict-card-wording implemented; remove plan/ledger` — flip spec Status to Implemented, delete plan.md and LEDGER.md.

## Per-commit tests

- Commit 1: `pnpm test` (vitest) — read real output. No test pins the old string (verified in spec); the suite must stay green.
- Prettier on the touched file before push if CI gates format.

## Risks

- Copy length/layout change on the card — cosmetic only; a11y suite (`pnpm test:a11y` coverage inside vitest run) covers the settings screen.
