//! One-off: run the Active Librarian over already-ingested vault documents.
//! Walks every indexed document in the brain DB and calls librarian::generate_summary,
//! which respects folder_rules (index = skip, summarize/synthesize = propose).
//!
//! Thin wrapper: the real flow lives in `cli_common::librarian_run` so
//! `ct librarian run` can call it too (Task 7). Default fallback model kept
//! exactly as before ("llama3.2:3b"; config overrides it in sidecar mode).
//! Watermark-gated: only dirty docs are synthesized; set FORCE_LIBRARIAN=1
//! to bypass the gate.
fn main() -> anyhow::Result<()> {
    let force = std::env::var("FORCE_LIBRARIAN").is_ok_and(|v| v == "1");
    curated_thoughts_tools::cli_common::librarian_run("llama3.2:3b", force)
}
