# `wisdom_gate` calibration fixtures (issue #265)

Cross-model paraphrase probes for calibrating `WISDOM_GATE_FLOORS`. The plan
(`docs/superpowers/plans/2026-10-06-issue265-wisdom-match.md`) Task 5 reads
these to compute a hit@2 / FP-rate sweep and pick a floor for the production
embedder, `external:qwen/qwen3-embedding-4b` (OpenRouter).

## Layout

- `facts.jsonl` — 100 developer-domain facts covering 10 topics (git, CI,
  Rust build, Python packaging, Docker, SQL/SQLite, HTTP APIs, testing,
  release/versioning, shell). Each row: `{id, title, body, source_type}`.
  - `id` runs `fact_wg_000`…`fact_wg_099`.
  - `source_type` is `librarian_inferred` for 90 and `user_stated` for 10.
- `probes.jsonl` — 200 short developer messages.
  - **100 relevant:** paraphrases of one fact's situation, written without
    quoting the fact body verbatim.
  - **100 irrelevant:** generic dev chat + vocabulary-sharing distractors
    that name the topic but ask something the fixtures do not cover.

## Provenance

- **Facts:** Claude. Two rows (`fact_wg_037`, `fact_wg_041`) originally used the key
  `source` instead of `body`; renamed to `body` on 2026-10-07 (content unchanged).
- **Probes:** GLM-5.3-FLASH via Z.AI's Anthropic-compatible endpoint
  (`https://api.z.ai/api/anthropic`), generated 2026-10-07 with the plan's prompts
  (Task 3 Step 2, reproduced below). Cross-model on purpose: facts by Claude, probes
  by GLM, so the gate is not tuned to one model's wording.
- **Hand review (plan Task 3 Step 3):** no relevant probe shares a 6-word run with
  its fact. Nine generated "irrelevant" probes were dropped because a fact answers
  them (cache key + OS → 010; queued concurrency → 019; WAL mode / checkpointing →
  050; `busy_timeout` → 051; manual VACUUM → 056; JSON generated-column / expression
  indexes ×2 → 058; brotli for static assets → 067). They were replaced by 9 new
  off-topic messages from the same GLM prompt so the calibrator's 200-probe minimum
  holds.

The validator asserts:

- `len(facts) >= 100`, all ids unique, every row exactly `{id, title, body, source_type}`
- `len(probes) >= 200`, at least 80 relevant and 80 irrelevant
- every probe's `expect` ids appear in the facts

## Regeneration

Never edit facts or probes by hand. To regenerate the probes with GLM
(`ZAI_API_KEY` must be set):

```python
import json, urllib.request, os
FACTS = [json.loads(l) for l in open("src-tauri/tests/fixtures/wisdom_gate/facts.jsonl")]
URL = "https://api.z.ai/api/anthropic/v1/messages"
HDR = {"x-api-key": os.environ["ZAI_API_KEY"], "anthropic-version": "2023-06-01",
       "content-type": "application/json"}
def ask(prompt):
    body = json.dumps({"model": "GLM-5.3-FLASH", "max_tokens": 4000,
                       "messages": [{"role": "user", "content": prompt}]}).encode()
    req = urllib.request.Request(URL, body, HDR)
    out = json.load(urllib.request.urlopen(req, timeout=120))
    return "".join(b.get("text", "") for b in out["content"])

probes = []
for f in FACTS:
    text = ask(
        "A developer is chatting with a coding agent. Write ONE message (1-2 sentences) "
        "the developer might send in a situation where the following fact would help, "
        "WITHOUT quoting it and avoiding its distinctive keywords where natural. "
        "Reply with the message only.\n\nFACT: " + f["title"] + " — " + f["body"])
    probes.append({"text": text.strip(), "expect": [f["id"]]})

# Irrelevant probes — 2026-10-07 WARNING: the plan's original wording here
# ("They must share vocabulary with these topics but ask something NONE of
# them answers") produced 9/100 probes that a fact actually answered
# (WAL mode, busy_timeout, VACUUM, JSON expression indexes, brotli…). GLM
# latches onto topic vocabulary and drifts into answered territory. Use the
# fully off-topic wording below, then still hand-check every "irrelevant"
# probe against the facts and drop any that returns a match under the
# calibrated floor.
topics = sorted({f["title"] for f in FACTS})
for batch in range(4):
    text = ask(
        "Write 25 short developer messages to a coding agent, one per line, no numbering. "
        "They must be about everyday development topics unrelated to git, CI, Rust, "
        "Python packaging, Docker, SQL, HTTP APIs, testing, releases or shell. "
        "Do not mention any of these topics either: "
        + "; ".join(topics))
    probes += [{"text": l.strip(), "expect": []} for l in text.splitlines() if l.strip()][:25]
with open("src-tauri/tests/fixtures/wisdom_gate/probes.jsonl", "w") as fh:
    for p in probes:
        fh.write(json.dumps(p, ensure_ascii=False) + "\n")

# Post-generation check (run before calibrating): no "irrelevant" probe may
# score at or above the calibrated floor against any fact vector.
import gzip
sweep = json.load(gzip.open("src-tauri/tests/fixtures/wisdom_gate/vectors.json.gz"))
floor = json.load(open("src-tauri/tests/fixtures/wisdom_gate/expected.json"))["floor"]
def cos(a, b):
    d = sum(x*y for x, y in zip(a, b)); na = sum(x*x for x in a) ** .5; nb = sum(x*x for x in b) ** .5
    return d / (na * nb) if na and nb else 0.0
bad = [p["text"] for p in probes if not p["expect"] and
       any(cos(p_vec, f_vec) >= floor for p_vec in [v["vector"] for v in sweep["probes"] if v["text"] == p["text"]]
           for f_vec in [v["vector"] for v in sweep["facts"]])]
print("probes that a fact answers — DROP AND REGENERATE:", bad)
```

## Calibration sweep

`calibrate_wisdom_gate --facts facts.jsonl --probes probes.jsonl --freeze .`
runs the floor sweep (0.20 → 0.90) on a scratch brain built from `facts`,
embeds everything with the **real** profile (default: OpenRouter
`qwen/qwen3-embedding-4b`, key from `OPENROUTER_API_KEY`), and writes `vectors.json.gz` + `expected.json`
here. `src-tauri/tests/wisdom_gate_bench.rs` (gated by
`--features slow-tests`) replays the frozen vectors through
`wisdom_match_with_floor` and asserts hit@2 has not regressed and FP ≤ 0.05.

`expected.json` records the SHA-256 of `facts.jsonl` and `probes.jsonl` so
the regression test fails closed if either file is edited under it. Editing either
file means regenerating the probes (if needed), rerunning the calibration, and
committing the new `expected.json` + `vectors.json.gz` together.

Current snapshot: floor **0.70**, hit@2 0.44, FP 0.05 —
`docs/benchmarks/2026-10-07-wisdom-gate-qwen3-embedding-4b.md`.