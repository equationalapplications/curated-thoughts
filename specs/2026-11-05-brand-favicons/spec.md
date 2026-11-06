# Curated Thoughts brand favicon + desktop icon set (ct-c-v6)

- **Date:** 2026-11-05
- **Status:** Draft — pending Kurt's spec approval (delivery flow Step 1)
- **Branch:** `assets/2026-11-05-brand-favicons`
- **Priority:** Low — cosmetic, no runtime behavior change

## Problem

The web UI still shows the default Vite icon (`index.html` links
`/vite.svg`), and the desktop bundle ships the default Tauri icon set in
`src-tauri/icons/`. Kurt approved the final brand asset set `ct-c-v6` on
2026-11-05 (`.ico` + 16/32/48/512 PNG + 180 apple-touch) and ruled: web
favicons into `public/` + `index.html`, and the desktop/Tauri app icon set
regenerated from the new 512px source.

## Approach

1. **Web favicons** — copy into `public/`:
   - `favicon.ico`, `favicon-16x16.png`, `favicon-32x32.png`,
     `favicon-48x48.png`, `apple-touch-icon.png` (180).
2. **`index.html`** — replace the `<link rel="icon" ... href="/vite.svg">`
   line with real links: `.ico`, 32px, 16px, and `apple-touch-icon` (180).
3. **Desktop icons** — regenerate the full `src-tauri/icons/` set with
   `pnpm tauri icon <source>` (local `@tauri-apps/cli` 2.12.3), source
   `favicon-ct-c-v6-512.png`. The CLI emits the exact filenames
   `tauri.conf.json` already references (`32x32.png`, `128x128.png`,
   `128x128@2x.png`, `icon.icns`, `icon.ico`, plus the Square*/StoreLogo
   Windows set), so `tauri.conf.json` needs no edit. Fallback if the CLI
   rejects a 512px source: upscale to 1024×1024 with PIL (Lanczos) first,
   regenerate, and note it in the ledger.
4. **AGENTS.md** — add the SDD enforcement block per the 2026-11-05
   pattern (this repo's first SOP PR): read-INTENT-first, authority chain,
   `specs/<name>/` folder, draft-until-implemented lifecycle, INTENT is
   static. CT's existing "Spec alignment" section stays.

### Non-goals

- No source-code changes; icons are data only.
- No Windows Store / macOS asset work beyond what `tauri icon` emits.

## Files touched

- `public/favicon.ico`, `public/favicon-16x16.png`,
  `public/favicon-32x32.png`, `public/favicon-48x48.png`,
  `public/apple-touch-icon.png` (all new)
- `index.html` (one block)
- `src-tauri/icons/*` (regenerated set)
- `AGENTS.md` (one block appended)

## Test plan

- `pnpm test` (vitest web suite) — run locally, read real output.
- Grep-verify `tauri.conf.json` icon paths are unchanged after
  regeneration.
- CI (`ci.yml`) green on the PR. Cargo suites are unaffected (no code
  change); CI's build covers the icon bundle.

## INTENT alignment

Checked `INTENT.md` 2026-11-05: brand assets; no intent conflict.
