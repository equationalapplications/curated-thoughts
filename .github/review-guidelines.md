# Review guidelines

- Report a defect only when you can trace the failure through code shown in this diff: name the concrete inputs or state, and the line where execution goes wrong. If the mechanism depends on code you have not seen, do not report it — "might" is not a finding.
- Never report a finding that your own analysis concludes is intended behaviour, is defended elsewhere in the diff, or is not a defect.
- Lines beginning with "-" in a patch hunk are removed code. Never base a claim on them — verify every mechanism against "+" and context lines only.
- If the diff adds a guard, fix, or handling for a problem, that problem is already fixed: do not report it as still present. Read the whole hunk before claiming something is missing.
- Comments in the diff are the author's statement of intent. Do not report as a defect any behaviour a comment in the diff documents as intentional.
- Clippy runs warn-only by design (spec docs/superpowers/specs/2026-09-04-ingest-integrity-wave-design.md §5, tracked in issue #175): `continue-on-error` on clippy steps is intentional, not a defect.
- `src-tauri` helpers like `load_strict_or_fresh` are private in-module read-modify-write helpers by design; the public surface is the setters. A private helper with all callers in-module is not an api-exposure defect.
- Integration tests set `CT_ALLOW_LIVE_BRAIN=1` deliberately when they pin the default `$HOME/.brain` resolution; a test carrying that opt-out is not a hermeticity defect.
- In Rust tests, a `TempDir` bound to a variable (`let t = TempDir::new()`) lives for the whole test; report a drop-before-use only when the handle is genuinely bound to a temporary statement.
- Do not report findings in test files (tests/, *_test.rs): defects in test code surface in CI, not production behaviour.
