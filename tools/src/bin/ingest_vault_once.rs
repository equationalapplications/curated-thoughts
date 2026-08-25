//! One-off: ingest every ingestible file in the configured vault into the brain DB.
//! Honors the pipeline's extension filter (code, docs, configs) and skips build
//! artifacts / VCS dirs. Symlinked directories directly under the vault root are
//! followed one level (spec 2026-05-05-second-brain-app-design.md L228).
//!
//! Thin wrapper: the real flow lives in `cli_common::ingest_run` so `ct ingest`
//! can call it too (Task 7).
fn main() -> anyhow::Result<()> {
    curated_thoughts_tools::cli_common::ingest_run()
}
