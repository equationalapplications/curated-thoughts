# Spec: Strict privacy card must be honest about embeddings (issue #355)

Status: Implemented

## Problem

The Strict privacy card claims the machine is fully sealed:

> "Fully local. Inference, embeddings, and storage all on-device. Cloud Bridge and external API fields disabled. Nothing ever leaves this machine."

This is not true today. Privacy mode governs LLM chat/generation and the cloud bridge only. The embedder path has no privacy gating: if a cloud embedding endpoint is configured, raw vault chunks are sent to it even in Strict mode.

## Evidence

- `src/components/privacy/PrivacyModeCards.tsx:12` — the Strict card summary makes the absolute claims above.
- `src-tauri/src/privacy/enforce.rs` — privacy enforcement gates only the cloud bridge and external generation; no privacy references exist on the embedding path.
- Enforcement follow-up for embeddings is tracked separately (issue #393); this PR fixes the copy only.

## Owner ruling (design approval)

Owner chat ruling, 2026-11-26 (verbatim): "Update the Strict card copy today so the UI is 100% honest about current behavior (governs LLM chat/generation and cloud bridge, not embeddings yet)."

## Proposed copy

New Strict card summary:

> "Chat and agent generation stay on-device, and Cloud Bridge is off. Cloud embeddings are not covered yet: if a cloud embedding endpoint is set, vault text can be sent to it."

Ephemeral and Connected card copy is unchanged.

## Sweep for the same over-claim

Grep of `src/` and user-facing `docs/` for "fully local", "nothing leaves this machine", "all on-device":

- `src/components/privacy/PrivacyModeCards.tsx:12` — fixed above.
- `docs/superpowers/specs/2026-07-05-ux-vision-okf-native-design.md:139` — historical design spec quoting the old copy; not user-facing UI. Left as-is (historical record).
- `docs/superpowers/specs/2026-08-28-strict-schema-ea-adoption-spec.md:123` — unrelated vault-boundary claim, not about privacy mode. Left as-is.

No frontend test asserts the old summary string (`src/__tests__/PrivacyPanel.test.tsx` asserts the "Strict (default)" label only, which is unchanged).

## Files touched

- `src/components/privacy/PrivacyModeCards.tsx` — Strict summary string only.

## Rejected alternatives

- Say "nothing leaves this machine except embeddings" — still an absolute framing; the owner ruling asks for plain honesty about what Strict governs.
- Gate embeddings now — out of scope; that is issue #393.
- Rewrite the Ephemeral/Connected cards — their copy already matches behavior.

## Test plan

- Run `pnpm test` (vitest) and read real output; confirm no test pinned the old string (verified above).
- Run prettier on the touched file if a format gate exists in CI.
