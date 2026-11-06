# Plan — ct-c-v6 favicons + Tauri icon regeneration

- **Date:** 2026-11-05
- **Spec:** `spec.md` (status: Reviewed)

1. **Web favicons.** Copy the 5 web files into `public/`; replace the
   `/vite.svg` link block in `index.html` with ico/32/16/apple-touch
   links; `rm public/vite.svg`. Verify dims + sha256 against sources.
   Risk: none.
2. **Desktop icons.** `pnpm exec tauri icon <512 source>` in the worktree
   (node_modules symlinked from the main checkout; no cargo build). If
   the CLI rejects the 512 source, upscale to 1024 with PIL Lanczos and
   note it. Verify `tauri.conf.json` icon paths unchanged (grep) and
   `git status` shows the regenerated set.
   Risk: low — pure image regeneration, no cargo compile.
3. **AGENTS.md.** Replace the ad-hoc "Spec alignment" section with the
   canonical SDD workflow block (one block, pattern 2026-11-05).
4. **Gates.** `pnpm test` (vitest web suite) — real output. Note for
   Kurt's local visual check: run Vite with `--force` to bust caches.
5. **Cleanup.** Delete `plan.md` + `LEDGER.md` in the same-PR cleanup
   commit; flip PR #315 out of draft.
