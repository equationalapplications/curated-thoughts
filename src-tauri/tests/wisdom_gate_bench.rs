//! Frozen-vector regression guard for the `ct wisdom match` floor (issue #265).
//! Recomputes hit@2 / FP through `wisdom_match_with_floor` from the vectors
//! the calibration runs froze. Guards the CODE PATH, not the model.
//!
//! Two snapshots, one per read scheme:
//! - `wisdom_gate/`          — raw scheme (cell A, floor 0.70)
//! - `wisdom_gate_instr1/`   — instr1 scheme (spec-rev2 cell E: BOTH sides take
//!   the byte-exact Qwen3 query instruction, floor 0.64)
#![cfg(feature = "slow-tests")]

use rusqlite::params;
use sha2::{Digest, Sha256};
use std::io::Read;
use tauri_app_lib::embed_scheme::Scheme;

const RAW_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wisdom_gate");
const INSTR1_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/wisdom_gate_instr1"
);

#[test]
fn wisdom_gate_floor_still_holds_raw() {
    replay(
        RAW_DIR,
        RAW_DIR,
        "external:qwen/qwen3-embedding-4b",
        Scheme::Raw,
    );
}

#[test]
fn wisdom_gate_floor_still_holds_instr1() {
    // Same probes/facts as raw (identical sha256s, re-pinned in instr1's
    // expected.json) — only the embedded text differs (both sides take the
    // byte-exact instruction), so the fixtures live once in RAW_DIR while the
    // freeze dir holds the instr1 vectors + expected.json.
    replay(
        INSTR1_DIR,
        RAW_DIR,
        "external:qwen/qwen3-embedding-4b:instr1",
        Scheme::Instr1,
    );
}

/// Projected two-stage composite floor key (spec §3 shape:
/// `embed_scheme::floor_key_for(model, scheme) + ":two-stage:" + rule`).
/// Won the 2026-10-18 live calibration (chunk-hop @ 0.65; see
/// `calibration-run/grid-20261018-100351/` and
/// docs/benchmarks/2026-10-18-wisdom-gate-two-stage-chunk-hop.md); flipped
/// into [`tauri_app_lib::wisdom_match::WISDOM_GATE_FLOORS`] by the flip PR.
const TWO_STAGE_GATE_KEY: &str = "external:qwen/qwen3-embedding-4b:two-stage:chunk-hop";

/// Floor key → freeze dir. The two calibrated v1 snapshots keep their
/// historical dir names (`wisdom_gate`, `wisdom_gate_instr1`); every key
/// calibrated later — the Task-15 composite two-stage key included — follows
/// the sanitized-key convention under `tests/fixtures/wisdom_gate_bench/`
/// (plan §Task 6).
fn freeze_dir_for(key: &str) -> String {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    match key {
        "external:qwen/qwen3-embedding-4b" => format!("{base}/wisdom_gate"),
        "external:qwen/qwen3-embedding-4b:instr1" => format!("{base}/wisdom_gate_instr1"),
        other => format!(
            "{base}/wisdom_gate_bench/{}",
            other.replace(':', "_").replace('/', "_")
        ),
    }
}

#[test]
fn every_non_stub_floor_key_has_a_freeze_dir() {
    // Stub keys are excluded by design: they live in test-only floor slices
    // (`WISDOM_GATE_FLOORS_TEST`, cfg(test)) and stub-backed dev runs — they
    // are never calibrated, so they never get a snapshot. Every real floor
    // row must have its freeze dir on disk: a floor can never ship without
    // the snapshot dir that justifies it.
    let missing: Vec<&str> = tauri_app_lib::wisdom_match::WISDOM_GATE_FLOORS
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| !k.starts_with("stub:"))
        .filter(|k| !std::path::Path::new(&format!("{}/expected.json", freeze_dir_for(k))).exists())
        .collect();
    assert!(
        missing.is_empty(),
        "WISDOM_GATE_FLOORS keys without a freeze dir under tests/fixtures: {missing:?} — \
         run `calibrate_wisdom_gate --freeze <DIR>` (or create the placeholder dir for a \
         not-yet-calibrated key, plan §Task 6)"
    );
}

#[test]
fn replay_two_stage() {
    let dir = freeze_dir_for(TWO_STAGE_GATE_KEY);
    let expected_path = format!("{dir}/expected.json");
    assert!(
        std::path::Path::new(&expected_path).exists(),
        "no two-stage freeze snapshot in {dir}; run the Task-15 paired calibration \
         and commit expected.json + vectors.json.gz"
    );
    let expected: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&expected_path).unwrap()).unwrap();
    let floor = expected["floor"].as_f64().unwrap() as f32;
    // THE flip-to-default tripwire: the composite-key lookup itself is
    // unchanged (spec §3) — until Task 15 adds the row, gate_floor misses
    // and this assert fails.
    assert_eq!(
        tauri_app_lib::wisdom_match::gate_floor(TWO_STAGE_GATE_KEY),
        Some(floor),
        "WISDOM_GATE_FLOORS[{TWO_STAGE_GATE_KEY}] must match the calibration snapshot in {dir}"
    );
    assert!(
        std::path::Path::new(&format!("{dir}/vectors.json.gz")).exists(),
        "partial freeze: expected.json and vectors.json.gz must be committed together"
    );
    println!(
        "two-stage freeze {} @ {floor}: rule {}, k {} ({} probes) — snapshot schema OK",
        TWO_STAGE_GATE_KEY,
        expected["rule"].as_str().unwrap(),
        expected["k"].as_u64().unwrap(),
        expected["probes"].as_array().map(|p| p.len()).unwrap_or(0),
    );
    // Task 15 extends this body: replay frozen["facts"]/["probes"] through
    // the two-stage path at expected["k"] under expected["rule"] with the
    // same hit@2 / FP assertions `replay` makes. The composite-key floor
    // assertion above is what gates this test on the calibration flip.
}

/// Issue #282 (spec rev-9 m3): a replay test FOR THE ACTIVITY COMPOSITE
/// KEY, parallel to [`replay_two_stage`], with its own sanitized-key freeze
/// dir. Un-ignored by the 2026-10-18 calibration flip: the grid evidence
/// dir + freeze snapshot landed (`calibration-run/grid-20261018-activity/`,
/// `docs/benchmarks/2026-10-18-wisdom-gate-activity-restricted-cosine.md`)
/// and the hit@1 letter passed in every kind, so the flip-gated floor
/// assertion below now bites exactly like [`replay_two_stage`] post-#287.
#[test]
fn replay_activity_two_stage() {
    let key = "external:qwen/qwen3-embedding-4b:activity:two-stage:restricted-cosine";
    let dir = freeze_dir_for(key);
    let expected_path = format!("{dir}/expected.json");
    assert!(
        std::path::Path::new(&expected_path).exists(),
        "no activity freeze snapshot in {dir}; run the #282 paired calibration \
         (calibrate_two_stage --query-kind ...) and commit expected.json + \
         vectors.json.gz"
    );
    let expected: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&expected_path).unwrap()).unwrap();
    let floor = expected["floor"].as_f64().unwrap() as f32;
    // The flip-to-default tripwire, activity edition: the composite-key
    // lookup itself is unchanged (spec §3.3) — until the calibrated activity
    // row lands, gate_floor misses and this assert fails.
    assert_eq!(
        tauri_app_lib::wisdom_match::gate_floor(key),
        Some(floor),
        "WISDOM_GATE_FLOORS[{key}] must match the calibration snapshot in {dir}"
    );
    assert!(
        std::path::Path::new(&format!("{dir}/vectors.json.gz")).exists(),
        "partial freeze: expected.json and vectors.json.gz must be committed together"
    );
}

/// Replays one frozen snapshot. `freeze_dir` holds vectors.json.gz +
/// expected.json; `fixtures_dir` holds facts.jsonl/probes.jsonl. `gate_key` is
/// the `WISDOM_GATE_FLOORS` key the snapshot was calibrated under — raw keys
/// are the bare model key, instr1 keys carry the `:instr1` suffix
/// (`embed_scheme::floor_key_for`). `scheme` is the cell the vectors were
/// frozen under: rows are stamped with it and the replay declares it, exactly
/// as `calibrate_wisdom_gate --scheme` did.
fn replay(freeze_dir: &str, fixtures_dir: &str, gate_key: &str, scheme: Scheme) {
    // The freeze files are produced by `calibrate_wisdom_gate --freeze <dir>`
    // against the real embedder (spec § "Calibration").
    // Until they land there is nothing to replay, but the skip stays fail-closed:
    // a half-written freeze, or a production floor without its snapshot, fails.
    let expected_path = format!("{freeze_dir}/expected.json");
    let vectors_path = format!("{freeze_dir}/vectors.json.gz");
    let have_expected = std::path::Path::new(&expected_path).exists();
    let have_vectors = std::path::Path::new(&vectors_path).exists();
    assert_eq!(
        have_expected, have_vectors,
        "partial freeze: expected.json and vectors.json.gz must be committed together"
    );
    if !have_expected {
        let uncovered: Vec<&str> = tauri_app_lib::wisdom_match::WISDOM_GATE_FLOORS
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| !k.starts_with("stub:"))
            .collect();
        assert!(
            uncovered.is_empty(),
            "WISDOM_GATE_FLOORS has {uncovered:?} but no calibration snapshot in {freeze_dir}"
        );
        eprintln!(
            "wisdom gate replay: SKIPPED (no freeze files in {freeze_dir}). Run \
             `calibrate_wisdom_gate --facts facts.jsonl --probes probes.jsonl --freeze <DIR>` \
             with OPENROUTER_API_KEY set and commit expected.json + vectors.json.gz."
        );
        return;
    }
    let expected: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&expected_path).unwrap()).unwrap();
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(&vectors_path).unwrap())
        .read_to_end(&mut raw)
        .unwrap();
    let frozen: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    // expected.json pins the fixture hashes: editing facts.jsonl or
    // probes.jsonl under a frozen snapshot fails until calibration is rerun.
    for (file, field) in [
        ("facts.jsonl", "facts_sha256"),
        ("probes.jsonl", "probes_sha256"),
    ] {
        let digest = hex::encode(Sha256::digest(
            std::fs::read(format!("{fixtures_dir}/{file}")).unwrap(),
        ));
        assert_eq!(
            Some(digest.as_str()),
            expected[field].as_str(),
            "{file} changed since calibration; rerun calibrate_wisdom_gate --freeze"
        );
    }
    let key = expected["model_key"].as_str().unwrap();
    let floor = expected["floor"].as_f64().unwrap() as f32;
    assert_eq!(
        tauri_app_lib::wisdom_match::gate_floor(gate_key),
        Some(floor),
        "WISDOM_GATE_FLOORS[{gate_key}] must match the calibration snapshot in {freeze_dir}"
    );

    let conn = tauri_app_lib::db::connection::open_in_memory().unwrap();
    let vec_of = |v: &serde_json::Value| -> Vec<f32> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_f64().unwrap() as f32)
            .collect()
    };
    for f in frozen["facts"].as_array().unwrap() {
        let blob: Vec<u8> = vec_of(&f["vector"])
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embed_scheme, embedding
             ) VALUES (?1, 'ent_calibration', ?2, ?3, '[]', 'inferred', ?4,
                       NULL, NULL, 100, 100, NULL, 0, NULL, ?5, ?6, NULL)",
            params![
                f["id"].as_str().unwrap(),
                f["title"].as_str().unwrap(),
                f["body"].as_str().unwrap(),
                f["source_type"].as_str().unwrap(),
                blob,
                scheme.as_str()
            ],
        )
        .unwrap();
    }
    let (mut hits, mut rel, mut fps, mut irr) = (0usize, 0usize, 0usize, 0usize);
    for p in frozen["probes"].as_array().unwrap() {
        let expect: Vec<&str> = p["expect"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        let m = tauri_app_lib::wisdom_match::wisdom_match_with_floor(
            &conn,
            &vec_of(&p["vector"]),
            key,
            scheme,
            Some(floor),
            2,
            &[],
            4_102_444_800_000,
        )
        .unwrap();
        if expect.is_empty() {
            irr += 1;
            fps += usize::from(!m.entries.is_empty());
        } else {
            rel += 1;
            hits += usize::from(m.entries.iter().any(|e| expect.contains(&e.id.as_str())));
        }
    }
    let hit = hits as f64 / rel as f64;
    let fp = fps as f64 / irr as f64;
    println!("wisdom gate {gate_key} @ {floor}: hit@2 {hit:.3}, FP {fp:.3}");
    assert!(fp <= 0.05, "FP {fp} > 0.05");
    assert!(
        hit >= expected["hit_at_2"].as_f64().unwrap() - 0.02,
        "hit@2 {hit} regressed below snapshot"
    );
}
