# Dispositions — #381 plan review, Opus r2 delta (APPROVE)

Repo: curated-thoughts, branch `fix/381-confirmations-hook-restart`.
Delta round over `opus-r1-dispositions.md`, reviewed head `1453f7e8`.
Verdict: **APPROVE** — "No significant issues." Cost $0.38 / 8 turns
(cap $1.50). T3 ladder total: $0.93 across 2 rounds.

Reviewer verified all four r1 folds against the code (M1's type and
visibility claims re-checked: hook type `mod.rs:96`, private slot but
same-module tests, `.map(|_| ())` type-checks like the current switch
block). Regression check on the changed text passed: all three call
sites canonicalize before `start_pipeline` (`lib.rs:2089–2093`,
`lib.rs:2379`, `lib.rs:4432–4434`); the direct-call test design cannot
race the worker; missing-ledger no-op re-confirmed
(`stated_ledger.rs:128–129`).

One implementer note (not a plan defect): tests 3–4 must create the
schema with `open_app_db` before seeding, as the harness does at
`mod.rs:1852`. ADOPTED into plan Task 2 as item 6.

Converged. Plan Status flips to "T3 CONVERGED — awaiting owner plan
approval".
