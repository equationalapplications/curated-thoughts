# Core LLM Wiki — pluggable vector retrieval (VectorRanker)

> **Status:** design spec (pre-implementation).  
> **Upstream tracking:** [equationalapplications/expo-llm-wiki#15](https://github.com/equationalapplications/expo-llm-wiki/issues/15)  
> **Downstream:** Curated Thoughts vault search today uses a separate Rust `semantic_search` full scan; this spec is for **`@equationalapplications/core-llm-wiki`** (`packages/core`) so React/Expo wiki memory can share ANN/sqlite-vec paths without duplicating policy.

---

## 1. Problem

`WikiMemory.read()` (in `packages/core/src/WikiMemory.ts`) loads candidate facts, parses `embedding_blob` (or legacy text embeddings), and scores with **`cosineSimilarity`** in JavaScript. Without MiniSearch **pre-filter**, that is a **full entity scan** — **O(number of facts)** per query, same complexity class as Curated Thoughts’ Rust chunk embedding search.

The in-memory **`vectorCache`** only avoids re-parsing blobs; it does **not** reduce asymptotic cost.

## 2. Goals

- Allow hosts to plug in **approximate or accelerated** retrieval (e.g. **sqlite-vec**, **sqlite-vss**, **USearch**, external vector DB) while **keeping embedding dimension / mismatch / `runReembed()` semantics** unchanged for callers.
- **Default behavior** when no plugin is configured: **today’s exact in-JS cosine path** (no regression).
- Preserve **hybrid** (semantic + MiniSearch) behavior where we commit to it (see §5).
- Keep **portability**: environments without native SQLite extensions must still run (fallback = current path).

## 3. Non-goals (v1)

- Shipping a **built-in** sqlite-vec extension inside `core` (host loads extension + provides ranker implementation).
- Changing **schema migrations** in core for a parallel vec table *unless* we add an **optional** migration behind an explicit host opt-in (defer to implementation plan).
- Unifying Curated Thoughts Rust DB and core wiki DB into one process (separate systems; **interface alignment** only).

## 4. Proposed concept: `VectorRanker`

Optional dependency injected via **`WikiOptions`**:

```typescript
// Conceptual — exact names in implementation plan
interface VectorRankerSearchArgs {
  entityId: string;
  queryVector: Float32Array; // length = current model dim; already validated vs meta
  /** Max number of fact **ids** to return (ranked best-first). */
  limit: number;
  /** SQL prefix for wiki tables, e.g. `wiki_` — ranker builds `${prefix}entries` if needed */
  tablePrefix: string;
  db: SQLiteAdapter;
  /**
   * When set (MiniSearch pre-filter path), ranker MUST restrict to these ids only.
   * When absent, ranker searches all non-deleted facts for the entity.
   */
  candidateIds?: string[];
}

interface VectorRanker {
  /**
   * Returns fact row ids with semantic scores (higher = better).
   * Scores SHOULD be cosine similarity on [-1, 1] when exact; approximate indexes MAY use other monotonic scores if documented.
   */
  searchSimilarFacts(args: VectorRankerSearchArgs): Promise<Array<{ id: string; score: number }>>;
}
```

**`WikiOptions`** gains:

- `vectorRanker?: VectorRanker`

## 5. Integration rules (`read()`)

### 5.1 When the ranker runs

Invoke **`vectorRanker.searchSimilarFacts`** only if **all** hold:

1. `options.vectorRanker` (from `WikiOptions`) is defined.
2. `embed()` succeeded and **dimension / mismatch checks** already passed (same guards as today — no behavior change before delegation).
3. **Pure semantic ranking** for this call: `hybridWeight` is **undefined** or **exactly `1`** after clamping (i.e. no keyword blend).  
   - **Rationale:** Hybrid blending (`weight < 1`) today needs **normalized MiniSearch scores for many rows**; delegating without a crisp contract would change ranking. **v1 fallback:** hybrid calls use the **legacy** blob + cosine path.

### 5.2 Pre-filter (`preFilterLimit`)

- If **`preFilterLimit`** produced a non-empty candidate id set → pass **`candidateIds`** to the ranker; ranker ranks **only** that set (hosts may ignore ANN and do exact cosine on the small set).
- If pre-filter returned **no** candidates → **no** ranker call; result is empty (same as today).
- If **`preFilterLimit`** is **unset** → full-entity semantic search; ranker implements “all facts for entity” (SQL, ANN index, or other).

### 5.3 After ranker returns

- **Phase 2** stays as today: fetch full fact rows **`SELECT * … WHERE id IN (…)`** in stable chunks for the top **`maxResults`** ids (or fewer if ranker returned fewer).
- **Tie-breaking:** If ranker scores collide, apply the **same deterministic tie-break** as the current cosine path: **access_count**, **updated_at**, **id** (lexicographic), using row fields from phase-2 (**not** relying on ranker order alone if we merge ties).

### 5.4 Cache (`vectorCache`)

- Ranker path **does not populate** `vectorCache` from full scans **v1** (avoids implying partial caches). Optionally document **`vectorRanker`** + **`preFilter`** as incompatible with warming the legacy cache → acceptable.

### 5.5 Errors

- Ranker **`throw`** or rejected Promise → treat like **`embed()` failure** for retrieval: fall back to **MiniSearch keyword** path and **`onRetrievalFallback`** if provided (reuse existing catch behavior around `read()`’s semantic block).

## 6. Write path (`embedFact` / `runReembed`)

**v1:** Ranker is **read-only**. Hosts that maintain an external index or sqlite-vec table are responsible for:

- **Upsert/delete** when facts embedding blobs change or rows delete, **or**
- Periodic **rebuild** job.

Document this clearly in WikiOptions JSDoc. **v2 (optional follow-up):** `VectorIndexSync` callback or hooks — **out of scope** for this spec unless product requires it.

## 7. Testing (acceptance-level)

- **Default:** existing core tests unchanged (no ranker).
- **With mock ranker:** fixed id order + synthetic scores → verify phase-2 selection, tie-break, and **`maxResults`** cap.
- **Hybrid `weight < 1`:** assert code path remains **legacy** (no ranker invocation).
- **Pre-filter + ranker:** `candidateIds` passed through (mock observes args).
- **Ranker throws:** fallback to keyword + optional `onRetrievalFallback` spy.

## 8. Documentation

- **`WikiOptions.vectorRanker`**: when used, hybrid limitation, write-path ownership, portability.
- **Issue #15** body in GitHub stays high-level; this file is the **normative design** for implementation.

## 9. Open questions (resolve before `writing-plans`)

| # | Question | Default if silent |
|---|----------|-------------------|
| Q1 | Should ranker scores be required to be **raw cosine**, or allow **monotonic proxies** with a flag? | Allow proxies; document per implementation |
| Q2 | **Cap** `limit` passed to ranker (`maxResults` vs `maxResults * oversample`)? | Pass **`maxResults`** only in v1; oversample = later optimization |
| Q3 | Should **Curated Thoughts** adopt the same **TypeScript interface** in a thin adapter for future bridge? | No code in this spec; optional follow-up |

---

## 10. Self-review (spec quality)

- **Contradictions:** None spotted; hybrid defers to legacy path by design.
- **Ambiguity:** Oversampling / ANN recall — explicitly deferred to v2.
- **Scope:** Core package only; no mandatory schema change in v1.

---

## 11. Next step (superpowers)

After you **approve** this spec, invoke **writing-plans** to produce  
`docs/superpowers/plans/YYYY-MM-DD-core-llm-wiki-vector-ranker.md` with file-level tasks against **`expo-llm-wiki/packages/core`** (exact paths, tests, commits).

**Please review this file and confirm or request edits before we generate the implementation plan.**
