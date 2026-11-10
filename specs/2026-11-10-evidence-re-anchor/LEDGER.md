# LEDGER — 2026-11-10-evidence-re-anchor

- 2026-11-10: WIP recovered from prior session's uncommitted worktree onto
  `feat/evidence-re-anchor` (spec + impl commits); origin/main merged clean.
  Draft PR #336 opened with gates held.
- 2026-11-10: Kurt adjudicated the three spec-vs-WIP deltas (backup KEPT;
  breaker/export dropped — WIP posture; verbatim-substring match — WIP
  posture). Spec revised to Reviewed. Plan T1 (backup) is the one remaining
  implementation gap.
- 2026-11-10 ~16:00: handoff
  `records/handoffs/2026-11-10-handoff-ct-evidence-re-anchor-repair.md`
  (Kurt: "own this"): live dry-run scored reanchored=0 BY CONSTRUCTION
  (median quote ~1.2k chars = whole old chunk; re-split leaves no chunk
  containing it). Match strategy re-adjudicated to normalized 60-char
  prefix + suffix-agreement; spec §3 round 2; matcher rewritten TDD (3 new
  tests; 14/14 module green).
- 2026-11-10: T1 DONE — `backup_before_apply` (`VACUUM INTO` →
  `brain.db.bak-pre-reanchor-<ts>`) wired into the apply path before the
  first UPDATE; dry-run takes no backup.
