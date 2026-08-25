//! One-off: approve all pending synthesize-mode proposals (memories/ folder).
//! Uses the real resolve_proposal commit path so outbox/events stay consistent.
//!
//! Thin wrapper: the real flows live in `cli_common::{approve_all, approve_one}`
//! so `ct approve` can call them too (Task 7).
fn main() -> anyhow::Result<()> {
    curated_thoughts_tools::cli_common::approve_all()
}
