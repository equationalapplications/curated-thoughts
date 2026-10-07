//! Frozen-vector regression guard for the `ct wisdom match` floor (issue #265).
//! Recomputes hit@2 / FP through `wisdom_match_with_floor` from the vectors
//! the calibration run froze. Guards the CODE PATH, not the model.
#![cfg(feature = "slow-tests")]

use rusqlite::params;
use sha2::{Digest, Sha256};
use std::io::Read;

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wisdom_gate");

#[test]
fn wisdom_gate_floor_still_holds() {
    // The freeze files are produced by `calibrate_wisdom_gate --freeze <dir>`
    // on the Linux reference machine with Ollama (spec § "Calibration").
    // Until they land there is nothing to replay, but the skip stays fail-closed:
    // a half-written freeze, or a production floor without its snapshot, fails.
    let expected_path = format!("{DIR}/expected.json");
    let vectors_path = format!("{DIR}/vectors.json.gz");
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
            "WISDOM_GATE_FLOORS has {uncovered:?} but no calibration snapshot in {DIR}"
        );
        eprintln!(
            "wisdom_gate_floor_still_holds: SKIPPED (no freeze files in {DIR}). Run \
             `calibrate_wisdom_gate --facts facts.jsonl --probes probes.jsonl --freeze <DIR>` \
             on the Linux reference machine and commit expected.json + vectors.json.gz."
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
            std::fs::read(format!("{DIR}/{file}")).unwrap(),
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
        tauri_app_lib::wisdom_match::gate_floor(key),
        Some(floor),
        "WISDOM_GATE_FLOORS must match the calibration snapshot"
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
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, 'ent_calibration', ?2, ?3, '[]', 'inferred', ?4,
                       NULL, NULL, 100, 100, NULL, 0, NULL, ?5, NULL)",
            params![
                f["id"].as_str().unwrap(),
                f["title"].as_str().unwrap(),
                f["body"].as_str().unwrap(),
                f["source_type"].as_str().unwrap(),
                blob
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
    println!("wisdom gate {key} @ {floor}: hit@2 {hit:.3}, FP {fp:.3}");
    assert!(fp <= 0.05, "FP {fp} > 0.05");
    assert!(
        hit >= expected["hit_at_2"].as_f64().unwrap() - 0.02,
        "hit@2 {hit} regressed below snapshot"
    );
}
