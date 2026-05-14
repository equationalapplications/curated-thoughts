## 🏗️ Phase 3: React & Desktop Optimization — Design Spec

**Date:** May 13, 2026
**Status:** Approved
**Branch:** `kv/fixes`
**Stack:** Tauri 2.x, React 19, `@equationalapplications/core-llm-wiki` v3.5.0+

### 1. Reactive Status Subscriptions

To prevent UI "jank" during heavy graph operations, the frontend must subscribe to real-time events from the Rust backend via the `subscribeEntityStatus` API.

**Implementation:** Create a unified `useWikiStatus` hook to track background jobs.

* **Monitored States:** `ingesting` (File Watcher), `librarian` (Synthesis), `healing` (Graph Repair).
* **UI Impact:** Disables the "Change Vault" selector and "Manual Re-index" buttons during active transactions to prevent `WikiBusyError`.

### 2. The `useMemoryRead` Reactive Hook

Phase 3 replaces standard search with a tiered, graph-aware reactive query. It utilizes the **Weighted Tiers** (Fact 1.5x, Wisdom 1.0x, Working 0.6x) and **Code Graph Expansion** from Phase 2.

* **Logic:** When a semantic match is found, the hook pulls "Structural Neighbors" (Callers/Callees) into the results.
* **Performance:** Implements a 300ms debounce and a **500-vector in-memory cache** per entity to ensure sub-millisecond results on desktop hardware.

### 3. Emoji-Safe Chunk Rendering

LLM-generated text often includes multi-byte characters (emojis, complex AST symbols) that break when split across chunks.

* **Requirement:** Use the `core-llm-wiki` "Emoji-Safe" chunking logic to ensure surrogate pairs are never split, preventing "broken character" UI bugs in the Review Queue.

### 4. Production Security & GDPR Compliance

As a local-first app, [Curated Thoughts](https://github.com/equationalapplications/curated-thoughts) must provide professional data hygiene.

* **Source Normalization:** Sanitizes file paths in the Rust backend to prevent path injection attacks.
* **Right to be Forgotten:** Implements the `runPrune` and `forget` methods.
* **"Prune Trash" Logic:** Hard-deletes `librarian_inferred` chunks that have been soft-deleted for more than 7 days, keeping the `brain.db` file lean.

---

## 🛠️ Phase 3 Deliverables

| Feature | Component | Purpose |
| --- | --- | --- |
| **Real-time Status** | `useWikiStatus.ts` | Prevents database collisions during vault switches. |
| **Tiered Highlighting** | `MemoryChunk.tsx` | Visually distinguishes **Facts** (Blue) from **Working** (Gray). |
| **Maintenance Dashboard** | `MaintenanceDashboard.tsx` | Provides manual controls for "Heal," "Prune," and "Full Re-index." |
| **Automatic Healing** | `Watcher.rs` | Triggers a 3s debounced `runHeal` after file deletions to remove "Ghost Notes." |

---

## 📈 Final System State

Upon completion of Phase 3, the [Curated Thoughts](https://github.com/equationalapplications/curated-thoughts) architecture fulfills the "Three-Tier Brain" vision:

1. **Fact Tier:** Immutable "North Star" truth from your `documents/` folder.
2. **Wisdom Tier:** Your curated, high-level wiki synthesis.
3. **Working Tier:** Structural awareness of your active code via the **Code Graph**.

**Next Step:** Are you ready to review the final integration test suite for the `mcp_integration` to verify the "Impact Radius" across all three tiers?