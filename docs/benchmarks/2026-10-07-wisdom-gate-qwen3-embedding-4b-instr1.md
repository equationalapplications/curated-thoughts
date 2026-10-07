# Wisdom gate calibration — `instr1` scheme, both-side instruction prefix (2026-10-07)

Calibration of the `ct wisdom match` abstention floor for the `instr1` embed
scheme (issue #265, spec
`docs/superpowers/specs/2026-10-07-issue265-wisdom-instruction-prefix-design.md`,
2×2 cell E: the byte-exact Qwen3 query instruction is prepended to BOTH the
probe (query) side and the fact (document) side — direct concatenation).

## What ran

```bash
cargo build --release --manifest-path tools/Cargo.toml --bin calibrate_wisdom_gate
./target/release/calibrate_wisdom_gate \
  --facts src-tauri/tests/fixtures/wisdom_gate/facts.jsonl \
  --probes src-tauri/tests/fixtures/wisdom_gate/probes.jsonl \
  --query-prefix 'Instruct: Given a web search query, retrieve relevant passages that answer the query
Query:' \
  --doc-prefix 'Instruct: Given a web search query, retrieve relevant passages that answer the query
Query:' \
  --freeze src-tauri/tests/fixtures/wisdom_gate_instr1
# regression replay of BOTH frozen snapshots (raw + instr1)
cd src-tauri && cargo test --features "test-utils,slow-tests" --test wisdom_gate_bench -- --nocapture
```

(The prefix is `embed_scheme::QUERY_INSTRUCTION_PREFIX`: one real newline
before `Query:`, no space after the colon, 91 bytes. The command above shows
it as a shell-quoted two-line string; the run took it verbatim from the source
constant.)

## Environment

- **Embedder:** OpenRouter `qwen/qwen3-embedding-4b` (2560-d), profile
  `{"type":"external","base_url":"https://openrouter.ai/api/v1","model":"qwen/qwen3-embedding-4b"}`
  — the production `embed_profile`. Both sides prefixed with the byte-exact
  instruction (`QUERY_INSTRUCTION_PREFIX`), i.e. 2×2 cell E.
- **Probe generator:** GLM-5.3-FLASH (Z.AI Anthropic-compatible endpoint), 2026-10-07.
  Facts by Claude. Fixtures are the SAME files as the raw calibration (the
  sha256s below are identical); only the embedded text differs.
- **Machine:** Ubuntu 26.04.1, Intel Core i5-2410M (2.30 GHz), 7 GB RAM.
- **Fixtures:** 100 facts; 200 probes (100 relevant, 100 irrelevant).
  - `facts.jsonl` sha256 `63c0349c8d95902769b50d9635c933108f33cfbc451fba3ae24caac714533f51`
  - `probes.jsonl` sha256 `6124b38562c5c6340a769b242bfa781397ffde7549d5ea78074af2f226a00ac1`

## Sweep (floor on raw cosine → hit@2, FP rate)

```
floor  hit@2   fp_rate
0.20   0.970   1.000
0.21   0.970   1.000
0.22   0.970   1.000
0.23   0.970   1.000
0.24   0.970   1.000
0.25   0.970   1.000
0.26   0.970   0.990
0.27   0.970   0.990
0.28   0.970   0.980
0.29   0.970   0.960
0.30   0.970   0.940
0.31   0.970   0.900
0.32   0.970   0.860
0.33   0.970   0.820
0.34   0.970   0.770
0.35   0.970   0.740
0.36   0.970   0.720
0.37   0.970   0.680
0.38   0.970   0.680
0.39   0.970   0.660
0.40   0.970   0.600
0.41   0.970   0.570
0.42   0.970   0.500
0.43   0.970   0.470
0.44   0.970   0.450
0.45   0.970   0.440
0.46   0.970   0.400
0.47   0.970   0.380
0.48   0.970   0.370
0.49   0.970   0.360
0.50   0.950   0.310
0.51   0.950   0.280
0.52   0.940   0.260
0.53   0.940   0.250
0.54   0.930   0.230
0.55   0.930   0.200
0.56   0.910   0.170
0.57   0.910   0.130
0.58   0.890   0.120
0.59   0.850   0.100
0.60   0.800   0.100
0.61   0.760   0.100
0.62   0.730   0.080
0.63   0.620   0.070
0.64   0.570   0.050
0.65   0.560   0.040
0.66   0.520   0.030
0.67   0.490   0.030
0.68   0.420   0.030
0.69   0.340   0.020
0.70   0.300   0.020
0.71   0.250   0.020
0.72   0.230   0.010
0.73   0.170   0.010
0.74   0.140   0.010
0.75   0.080   0.010
0.76   0.080   0.010
0.77   0.080   0.000
0.78   0.050   0.000
0.79   0.050   0.000
0.80   0.020   0.000
0.81   0.020   0.000
0.82   0.010   0.000
0.83   0.000   0.000
0.84   0.000   0.000
0.85   0.000   0.000
0.86   0.000   0.000
0.87   0.000   0.000
0.88   0.000   0.000
0.89   0.000   0.000
0.90   0.000   0.000
```

## Result

- **Chosen floor:** 0.64 (max hit@2 subject to FP ≤ 0.05; ties → higher floor).
- **hit@2 0.57, FP 0.05.** Both-sides-prefixed (cell E) beats the raw scheme
  at its floor on recall (0.57 vs 0.44 at equal FP 0.05) and lifts the ceiling
  (0.97 vs 0.94 peak hit@2).
- The gate stays conservative: below 0.64 the FP rate climbs fast (0.07 at
  0.63, 0.10 at 0.59) while hit@2 recovers to 0.97 only at ≤ 0.49.
- The frozen snapshot pins `external:qwen/qwen3-embedding-4b:instr1` → floor
  0.64 in `WISDOM_GATE_FLOORS`; **the raw scheme numbers are unchanged**
  (window semantics: `raw` reads stay on the raw snapshot, floor 0.70,
  hit@2 0.44 — see `docs/benchmarks/2026-10-07-wisdom-gate-qwen3-embedding-4b.md`).
  The full live replay run is pasted at the end.

## Regression replay (local run, 2026-10-07)

```
$ cd src-tauri && CARGO_TARGET_DIR=../ct-target cargo test -j 2 --features test-utils,slow-tests --test wisdom_gate_bench -- --nocapture

running 2 tests
wisdom gate external:qwen/qwen3-embedding-4b:instr1 @ 0.64: hit@2 0.570, FP 0.050
test wisdom_gate_floor_still_holds_instr1 ... ok
wisdom gate external:qwen/qwen3-embedding-4b @ 0.7: hit@2 0.440, FP 0.050
test wisdom_gate_floor_still_holds_raw ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.67s
```
