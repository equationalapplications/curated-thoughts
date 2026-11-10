# Plan — llm-wiki 8.0.2 bump (spec: 2026-11-10-llm-wiki-8-bump)

Pre-flight (done during investigation): `df -h` ≥30G on the build-cache
filesystem (99G free); no Rust work in a parallel wave; spec facts read
from the real 7.13.0/8.0.2 tarballs, not memory.

1. **Branch** — `deps/llm-wiki-8-0-2` off current `main` (`705a7be7`).
2. **Version pins** — `package.json`: four `dependencies` entries
   (`core-llm-wiki`, `react-llm-wiki`, `schema-org-llm-wiki`,
   `schema-software-org`) → `8.0.2`; `pnpm.overrides` `core-llm-wiki` →
   `8.0.2`, `core-okf` `7.9.0` → `8.0.2`.
   `src-tauri/src/db/schema_guard.rs:8` `PINNED_CORE_LLM_WIKI_VERSION` →
   `"8.0.2"`.
3. **Behavior pins** — `src/lib/wiki.ts` `makeWikiOptions`: add
   `librarian: { strategy: 'legacy' }` and `maintenance: 'auto'` under
   `config` with the spec's rationale comment; update the three 7.13.0
   host-contract comment stamps to "introduced 7.13.0, reverified 8.0.2".
4. **Install** — `CI=true npx -y pnpm@10.33.2 install --no-frozen-lockfile`
   (global pnpm is 9.15.9; bare `pnpm install` prompts — #208 gotcha).
   Confirm the lockfile moves only the five packages + core-okf.
5. **Check test constructors** — grep `createWiki(` call sites in
   `src/` and tests; any site that does NOT route through
   `makeWikiOptions` needs the same explicit pin or a route through it,
   so no suite silently exercises 8.x defaults.
6. **Frontend gates** — `pnpm run typecheck`, `pnpm run lint`, `pnpm test`.
   Known-local-flake set (node 26): `useProposalNotifications` ×5,
   `SettingsScreen` ×1 — pre-existing on main, not a regression signal.
   Note: local lint may flag `.cargo-shared-target` artifacts; that is the
   known false positive, not a finding.
7. **Rust gates (serial)** — export
   `CARGO_TARGET_DIR=<repo>/.cargo-shared-target`; run
   `cargo test --features test-utils,mcp-server`. Must-pass set:
   `rust_llm_wiki_ddl_matches_core_llm_wiki_package`, `schema_guard::*`,
   `engine_source_ref_gate` (after step 4's install).
8. **Stamp sweep** — `grep -rn "7\.13\.0"` over tracked source; only
   historical references may remain.
9. **Commits** — (a) spec+plan+LEDGER; (b) implementation; (c) cleanup
   commit deleting `plan.md` + `LEDGER.md` (same-PR cleanup, AGENTS.md).
   Conventional style, `chore(deps):` for (b) per #208 precedent; no
   squash merge later (regular `--merge`).
10. **Push + PR** — open against `main`, body summarizing spec §Change and
    gate results; PR stays open for the two independent AI reviews (GLM +
    Opus) per INTENT §3. Update spec Status → `Implemented` before the
    cleanup commit.
