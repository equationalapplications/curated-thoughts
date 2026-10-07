# Wisdom gate calibration — `external:qwen/qwen3-embedding-4b` (2026-10-07)

Calibration of the `ct wisdom match` abstention floor (issue #265, spec
`docs/superpowers/specs/2026-10-06-issue265-wisdom-match-design.md` § "Calibration").

## What ran

```bash
cargo build --release --manifest-path tools/Cargo.toml --bin calibrate_wisdom_gate
./target/release/calibrate_wisdom_gate \
  --facts src-tauri/tests/fixtures/wisdom_gate/facts.jsonl \
  --probes src-tauri/tests/fixtures/wisdom_gate/probes.jsonl \
  --freeze src-tauri/tests/fixtures/wisdom_gate
# regression replay of the frozen vectors
cd src-tauri && cargo test --features "test-utils,slow-tests" --test wisdom_gate_bench -- --nocapture
```

## Environment

- **Embedder:** OpenRouter `qwen/qwen3-embedding-4b` (2560-d), profile
  `{"type":"external","base_url":"https://openrouter.ai/api/v1","model":"qwen/qwen3-embedding-4b"}`
  — the production `embed_profile`. Raw text, no query instruction prefix.
- **Probe generator:** GLM-5.3-FLASH (Z.AI Anthropic-compatible endpoint), 2026-10-07.
  Facts by Claude.
- **Machine:** macOS 15.8.1, Intel Core i7-8559U.
- **Fixtures:** 100 facts; 200 probes (100 relevant, 100 irrelevant).
  - `facts.jsonl` sha256 `63c0349c8d95902769b50d9635c933108f33cfbc451fba3ae24caac714533f51`
  - `probes.jsonl` sha256 `6124b38562c5c6340a769b242bfa781397ffde7549d5ea78074af2f226a00ac1`

## Sweep (floor on raw cosine → hit@2, FP rate)

```
floor  hit@2   fp_rate
0.20   0.940   1.000
0.21   0.940   1.000
0.22   0.940   1.000
0.23   0.940   1.000
0.24   0.940   1.000
0.25   0.940   1.000
0.26   0.940   1.000
0.27   0.940   1.000
0.28   0.940   0.990
0.29   0.940   0.990
0.30   0.940   0.990
0.31   0.940   0.980
0.32   0.940   0.960
0.33   0.940   0.950
0.34   0.940   0.920
0.35   0.940   0.920
0.36   0.940   0.890
0.37   0.940   0.850
0.38   0.940   0.810
0.39   0.940   0.760
0.40   0.940   0.740
0.41   0.940   0.700
0.42   0.940   0.650
0.43   0.940   0.610
0.44   0.940   0.600
0.45   0.940   0.580
0.46   0.940   0.560
0.47   0.940   0.520
0.48   0.940   0.510
0.49   0.940   0.490
0.50   0.940   0.470
0.51   0.940   0.450
0.52   0.920   0.430
0.53   0.920   0.400
0.54   0.920   0.390
0.55   0.890   0.370
0.56   0.880   0.340
0.57   0.870   0.310
0.58   0.860   0.270
0.59   0.850   0.260
0.60   0.840   0.250
0.61   0.820   0.240
0.62   0.770   0.220
0.63   0.750   0.200
0.64   0.730   0.150
0.65   0.710   0.130
0.66   0.650   0.120
0.67   0.610   0.100
0.68   0.540   0.100
0.69   0.480   0.090
0.70   0.440   0.050
0.71   0.370   0.040
0.72   0.310   0.030
0.73   0.260   0.020
0.74   0.230   0.020
0.75   0.200   0.020
0.76   0.160   0.020
0.77   0.140   0.010
0.78   0.140   0.010
0.79   0.080   0.010
0.80   0.060   0.000
0.81   0.050   0.000
0.82   0.040   0.000
0.83   0.010   0.000
0.84   0.010   0.000
0.85   0.010   0.000
0.86   0.000   0.000
0.87   0.000   0.000
0.88   0.000   0.000
0.89   0.000   0.000
0.90   0.000   0.000
```

## Result

- **Chosen floor:** 0.70 (max hit@2 subject to FP ≤ 0.05; ties → higher floor).
- **hit@2 0.44, FP 0.05.** The bench replays the frozen vectors to the same numbers.
- The gate is conservative: below 0.70 the FP rate climbs fast (0.09 at 0.69,
  0.25 at 0.60) while hit@2 recovers to 0.94 only at ≤ 0.50. Relevant and
  irrelevant probes overlap heavily on raw cosine for this model.

## Not measured

- **Latency** (plan Task 5 Step 4: 50 `ct wisdom match` calls against a scratch
  copy of a real brain, target p95 ≤ 1.5 s warm) — not run in this pass.
