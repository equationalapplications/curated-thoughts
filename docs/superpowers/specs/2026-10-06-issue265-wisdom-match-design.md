# `ct wisdom match`: relevance-gated, read-only wisdom match (issue #265)

**Date:** 2026-10-06
**Status:** implemented; calibrated 2026-10-07 for `external:qwen/qwen3-embedding-4b` (floor 0.70, hit@2 0.44, FP 0.05 — `docs/benchmarks/2026-10-07-wisdom-gate-qwen3-embedding-4b.md`). Latency measured 2026-10-07 on Linux: p50 0.44 s / p95 1.21 s warm over 50 calls (target ≤ 1.5 s — pass).
**Branch:** `feat/issue-265-wisdom-match`
**Issue:** #265
**Consumer:** curated-thoughts-integrations (CTI) live wisdom delivery. CTI spec
`docs/superpowers/specs/2026-10-06-intuitive-wisdom-live-delivery-design.md`
§"CT prerequisite" defines the contract below. CTI ships its side first against a faked
`ct`; only its e2e run waits on this.

Investigation: `2026-10-06-issue265-wisdom-match-investigation.md` (same directory);
every `[V]` cited below has its evidence there.

## Problem

CTI will ask CT, on each user turn of an agent session, which curated wisdom facts are
relevant to the user's message, and deliver only those. CTI's INTENT forbids
integrations from scoring or ranking, so the relevance decision belongs to CT, and so
does CT INTENT rule 7 ("recall abstains; it does not bluff").

Nothing in CT does this today:

- `ct recall` exits 2 when the chunk leg is empty. Its wiki leg is unscored lexical
  overlap with no threshold, and it returns superseded rows [V Q1].
- `wiki_graph::wiki_search` is semantic, but it has no abstention floor and no
  current-only filter [V Q2].
- Nothing in Rust reads the V24 supersession columns [V Q4].

## Contract (shared with CTI; changing it needs a coordinated CTI change)

```
ct wisdom match --json [--max N] [--exclude=<id>]... -- <text>
```

| Argument | Rule |
|---|---|
| `<text>` | Required, and only accepted after `--` (clap `last = true`). Up to 2000 chars are used; CT truncates silently at a char boundary. Empty or whitespace-only → success with zero matches |
| `--json` | Required for the machine form. Without it, a human-readable listing (one line per item) |
| `--max N` | Default 2, clamped to `0..=10`. Bounds `entries` only. `--max 0` = corrections only |
| `--exclude=<id>` | Repeatable. Each value must match `^[A-Za-z0-9._:-]{1,128}$`, else a usage error. At most 1024 values, else a usage error |

stdout with `--json`:

```json
{
  "schema": 1,
  "gate": "semantic-v1:external:qwen/qwen3-embedding-4b",
  "entries": [
    {"id": "fact_…", "title": "…", "text": "…", "score": 0.71,
     "supersedes": [], "provenance": "librarian_inferred"}
  ],
  "corrections": [
    {"id": "fact_…", "title": "…", "text": "…", "score": null,
     "supersedes": ["fact_old…"], "provenance": "user_stated"}
  ]
}
```

| Case | Exit | Output |
|---|---|---|
| Success, including zero matches and the `uncalibrated` gate | 0 | JSON |
| Brain cannot be resolved or opened, or the embed profile cannot be loaded | 1 | Message on stderr |
| Query embedding fails (backend down, timeout) | 1 | Message on stderr |
| Usage error (missing `--`, bad `--exclude` id, too many excludes) | 1 | Message on stderr. `ct`'s `main` maps every parse error to 1 (`tools/src/bin/ct.rs`), and exit 2 already means "no results" (`EXIT_NO_RESULTS`); `wisdom match` never returns 2 |

`ct wisdom match --help` exits 0. CTI uses it as the capability probe.

## Design

### Units

| Unit | Location | Responsibility |
|---|---|---|
| `wisdom_match` (+ `wisdom_match_with_floor` for tests and calibration) | new `src-tauri/src/wisdom_match.rs`, a sibling of `wiki_graph.rs` (already 1,908 lines); reuses `search::{bytes_to_f32, cosine_similarity}` and `wiki_graph::tier_weight` | Pure, read-only. `(conn, query_vec, gate_key, max, exclude, now_ms) -> Result<WisdomMatch>` |
| `gate_model_key(&EmbedProfile, stub: Option<&str>) -> String` | `src-tauri/src/wisdom_match.rs` (the stub value is passed in, so it is testable without env) | Gate key |
| `WISDOM_GATE_FLOORS` + `gate_floor(key) -> Option<f32>` | `src-tauri/src/wisdom_match.rs` | Floor table and lookup |
| `ct wisdom match` | `tools/src/bin/ct.rs` (new `Match` variant in the **existing** `WisdomCmd` group, next to `deposit` / `status` / `propose-supersession` / `pending`) + `tools/src/queries.rs` (`wisdom_match_cmd`) | Argument parsing, brain/profile resolution exactly like `run_query`, embedding the text, calling `wisdom_match`, printing |
| `calibrate_wisdom_gate` | `tools/src/bin/calibrate_wisdom_gate.rs` | Calibration on a scratch brain (see "Calibration") |

`ct recall` and MCP `wiki_search` are unchanged.

### Gate key

`gate_model_key` returns `"local:<model>"`, `"cloud:<provider>:<model>"` or
`"external:<model>"`, all lowercased. When `CURATED_EMBED_STUB` is set, it returns
`"stub:<value>"` instead, because the stub replaces the embedding regardless of the
profile.

The `gate` output field is `"semantic-v1:" + key` when the key has a floor, and
`"uncalibrated"` otherwise.

### Floor table

```rust
/// Abstention floors on RAW cosine, one per embedding model (CT INTENT rule 7).
/// Values come only from a `calibrate_wisdom_gate` run recorded under
/// docs/benchmarks/. A model not listed here abstains.
pub const WISDOM_GATE_FLOORS: &[(&str, f32)] = &[
    ("stub:constant8", 0.5), // test-only key: unreachable without CURATED_EMBED_STUB
    ("external:qwen/qwen3-embedding-4b", 0.70), // calibrated 2026-10-07
];
```

The production entry is added by the plan's calibration task, together with its
`docs/benchmarks/` snapshot. Until then, real brains report `"gate": "uncalibrated"`
and return no `entries`. That is the safe order to ship in: CTI degrades to nothing,
never to noise.

### Matching (`entries`)

1. Read the columns present on `llm_wiki_entries` (`ddl_compat::existing_columns`). A
   read-only open of a brain that predates V24 lacks `superseded_by` and `valid_to`.
   Those filters are then omitted, and `corrections` is empty, since nothing can be
   superseded.
2. Select the candidates:

   ```sql
   SELECT id, entity_id, title, body, source_type, embedding_blob
   FROM llm_wiki_entries
   WHERE deleted_at IS NULL AND embedding_blob IS NOT NULL
     [AND superseded_by IS NULL]
     [AND (valid_to IS NULL OR valid_to > ?now_ms)]
   ```

   `now_ms` is a parameter, so tests are deterministic. The CLI passes the wall clock
   in epoch milliseconds, the unit the engine writes [V Q4].
3. Skip ids in the exclude set (a Rust `HashSet`, never an SQL `IN` list) and blobs
   whose length is not `dim * 4`.
4. `raw = cosine(query, blob)`, reusing `wiki_graph`'s existing helpers. Keep the row
   only when `raw >= floor`. The floor compares **raw** cosine, never the tier-weighted
   score, so provenance is never a hidden ranking penalty (CT INTENT non-goal).
5. Order by `raw * tier_weight(entity_id)` descending, then `id` ascending as a
   deterministic tiebreak. This is the same ordering `wiki_search` uses. Truncate to
   `max`.
6. Emit `score = raw` (f32, serialized as a JSON number), `supersedes = []`,
   `text = body`.

With no floor for the key, steps 2–6 are skipped and `entries` is `[]`.

### Corrections

For each distinct `--exclude` id, in the order given:

1. Look up `superseded_by` for the id. A missing row, or `superseded_by IS NULL`, means
   no correction.
2. Follow `superseded_by` forward, tracking visited ids. Stop when the depth exceeds
   100 (the engine's `HISTORY_MAX_DEPTH` [V Q4]) or an id repeats; in both cases there
   is no correction.
3. The **head** is the first row that has `superseded_by IS NULL`, `deleted_at IS NULL`
   and `valid_to` NULL or greater than `now_ms`. If the chain reaches a deleted row
   first, there is no correction.
4. If the head is itself in the exclude set, the caller already holds the replacement,
   so there is no correction.
5. Several excluded ids that resolve to the same head produce **one** correction whose
   `supersedes` lists all of them, in input order.

**One id, one list.** Corrections are computed before matching. A correction head
that would also pass the gate is removed from the `entries` candidates before step 5's
truncation, so it appears **only** in `corrections`, where it carries its `supersedes`
marker. `entries` then fills up to `max` from the remaining candidates. No id ever
appears twice in one response.

A correction is emitted whether or not it matches `<text>`. `score` is `null` and it
does not need an embedding. Corrections are not counted against `--max`, and are
ordered by the first excluded id that produced each one. The cost is one indexed
lookup per chain step: `id` is the primary key.

**Dependency (not in scope):** `superseded_by` is written only by the engine's
`supersede` [V Q4]. `corrections` stays empty in practice until the Active Librarian
applies supersession deposits through that path (CT INTENT rule 4, decided). The plan
records this as a follow-up issue; this spec only reads the columns.

### Provenance

`provenance` is the stored `source_type` when it is one of `librarian_inferred`,
`user_stated`, `user_confirmed` or `immutable_document` (CT's vocabulary, owned here);
any other value or NULL gives `null`. When attestation (INTENT rule 2) lands, the set
grows in CT, and consumers treat unknown strings as labels. Today's data has no human
or agent class, so CT does not claim one.

### Calibration (`calibrate_wisdom_gate`)

The tool never touches the live brain (CT INTENT workflow 5). It builds its own scratch
brain in a temp dir from fixtures:

- `--facts tools/tests/fixtures/wisdom_gate/facts.jsonl`: ≥ 100 developer-domain facts,
  each `{id, title, body, source_type}`. The tool creates a fresh brain with the app
  schema (`AppDb`), inserts the rows, and embeds them with the **real** profile under
  test (`--profile <json>`, default the production embedder, OpenRouter `qwen/qwen3-embedding-4b`). It refuses to
  run when `CURATED_EMBED_STUB` is set.
- `--probes tools/tests/fixtures/wisdom_gate/probes.jsonl`: ≥ 200 probes, each
  `{text, expect: [fact ids]}`. They are split roughly evenly:
  - **relevant:** paraphrases of a fact's situation, written by a different model than
    the facts (the generator is recorded in the fixture header). This satisfies the
    "cross-model paraphrase probes" requirement.
  - **irrelevant:** `expect: []`. These are developer messages the facts do not cover,
    including near-miss distractors that share vocabulary with a fact.
- **Sweep:** floors from 0.20 to 0.90 in steps of 0.01. For each floor it computes
  **hit@2**, the share of relevant probes with an expected id in the top 2 entries, and
  **FP rate**, the share of irrelevant probes that return any entry.
- **Selection rule:** the floor that maximizes hit@2 subject to FP rate ≤ 0.05. Ties go
  to the higher floor. If no floor meets the FP bound, the tool reports failure and the
  model stays uncalibrated.
- **Output:** a table on stdout plus `--freeze <dir>`, which writes gzip'd fact and
  probe vectors next to the fixtures, following the existing bench convention
  (`docs/benchmarks/README.md`).
- **Regression guard:** a `slow-tests` test (`src-tauri/tests/wisdom_gate_bench.rs`)
  recomputes both metrics from the frozen vectors through `wisdom_match` itself. It
  asserts that the table's floor still meets FP ≤ 0.05 and that hit@2 has not fallen
  more than 0.02 below the value recorded in the benchmark snapshot. This guards the
  code path, not the model.
- Every calibration run is recorded as `docs/benchmarks/YYYY-MM-DD-wisdom-gate-<model>.md`:
  the model, fixture hashes, the sweep table, the chosen floor and the reference
  machine.

## Error handling

- `wisdom_match` returns `Result`. A malformed row (unreadable column, NULL title or
  body) is skipped with a stderr note, matching `rank_wiki_entries`' rule that one bad
  row never fails the call.
- The database is opened read-only via the existing `open_ro`. All SQL uses bound
  parameters. Exclude ids never reach SQL text.
- The CLI maps failures to the exit codes in the contract. stdout carries JSON only on
  exit 0.

## Testing

**Unit tests** (`wiki_graph`, in-memory database with crafted unit vectors):

- floor: an entry just below the floor is dropped and one at the floor is kept;
- the gate uses raw cosine: a `tier_fact` row at 0.9× the floor is still dropped;
- exclude: an excluded id never appears in `entries`;
- current-only: rows with `superseded_by` set, rows with `valid_to <= now`, and deleted
  rows are dropped; `valid_to > now` is kept;
- pre-V24 table without the temporal columns: matching works and `corrections` is
  empty;
- ordering: tier weight orders results above the floor; equal scores break ties by id;
  truncation to `max`; `max = 0`;
- dimension mismatch is skipped;
- uncalibrated key: `gate = "uncalibrated"` and no entries, while corrections still flow;
- corrections: a single hop; a multi-hop chain to its head; a chain ending in a deleted
  row; a cycle; depth > 100; a head already excluded; two excludes to one head merged;
  a non-superseded exclude; an unknown id;
- one id, one list: a correction head that also clears the floor appears only in
  `corrections`, and `entries` backfills to `max` from the next candidate;
- provenance mapping, including unknown values and NULL;
- `gate_model_key` for each profile variant and for the stub.

**CLI tests** (`tools/tests/ct_wisdom_match.rs`, `with_seeded_brain` +
`CURATED_EMBED_STUB=constant8`):

- the JSON schema keys, and exit 0 with zero matches;
- `--` is required (exit 2 without it); a text starting with `-` is accepted after `--`;
- a bad `--exclude` id gives exit 2; 1025 excludes give exit 2;
- an empty text gives exit 0 with empty lists;
- `--help` exits 0;
- an unresolvable brain gives exit 1 with a stderr message.

**Benchmark:** the calibration run plus the frozen-vector regression test above.

**Latency:** p95 ≤ 1.5 s warm over 50 calls with
`external:qwen/qwen3-embedding-4b`, recorded in the benchmark snapshot. The candidate scan is
O(live embedded entries × dim): about 7.7 M multiply-adds at 10⁴ × 768, small next to
the query embedding call.

## Known limitations

1. No per-entry model stamp on `embedding_blob` [V Q3]. After an embed-model swap that
   keeps the dimension, scores are meaningless until the entries are re-embedded. The
   floor table can't detect this. This is pre-existing for `wiki_search` too.
2. `corrections` stays empty until the Librarian applies supersessions (see
   Dependency).
3. Work scope (INTENT rule 8) isn't applied, because no recall path implements it yet.
   When it lands, `wisdom match` takes the same scope filter as other recall.

## Out of scope

- A System One judge. It can be added later inside `wisdom_match` behind the same
  contract (INTENT rule 6: optional).
- "Questions this fact answers" multi-vector recall (INTENT "Active Librarian
  functions").
- An MCP tool for this, and any change to `ct recall`, `wiki_search` or `wiki_context`.
- Writing `superseded_by` (the Librarian's job).

## Versioning

`feat:` commits trigger a minor release through the existing semantic-release flow.
CTI's README records the minimum CT version that ships `ct wisdom match`, once it is
released.
