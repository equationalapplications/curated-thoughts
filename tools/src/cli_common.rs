//! Re-export shim for the split modules.
//!
//! Phase 2 split (see spec docs/superpowers/specs/2026-08-25-ct-headless-cli-phase2-watch.md):
//!   - `paths`   — BrainPaths, resolve_brain_paths, print_json, vault_contains
//!   - `queries` — read-only subcommand fns (status, search, recall, code, graph, wiki_*)
//!   - `cmds`    — write subcommand fns (ingest, librarian, approve, watch, enqueue_vault_event)
//!   - `write`   — DB write helpers (Brain, open_ro, open_rw, resolve, EXIT_NO_RESULTS)
//!
//! Existing `use curated_thoughts_tools::cli_common::X` paths in
//! tools/src/bin/* and the integration tests keep compiling via these
//! `pub use`s. A follow-up PR will remove the re-exports once all
//! consumers migrate to direct module paths.

pub use crate::cmds::*;
pub use crate::paths::*;
pub use crate::queries::*;
pub use crate::write::*;
