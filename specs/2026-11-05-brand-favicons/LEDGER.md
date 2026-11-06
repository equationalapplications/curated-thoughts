# LEDGER — 2026-11-05-brand-favicons (curated-thoughts)

What's in flight: implementation on branch `assets/2026-11-05-brand-favicons`,
PR #315 (draft → ready after cleanup commit).

- [x] 1. Web favicons in public/ + index.html links + vite.svg removed
       (dims + sha256 verified) — 5/5 sha-match: ico 48², 16², 32²,
       48² RGBA, apple-touch 180²
- [x] 2. Tauri icons regenerated; tauri.conf.json paths unchanged —
       16 icons modified, +android/ +ios/ +64x64.png emitted by CLI
       (all committed; no stray untracked artifacts)
- [x] 3. AGENTS.md canonical SDD block — replaced ad-hoc "Spec alignment"
- [x] 4. Gates: pnpm test — Test Files 96 passed (96); Tests 618 passed,
       1 skipped (619); two full runs exit 0
- [ ] 5. Cleanup commit (plan/LEDGER deleted); PR ready
