//! Structural guard on the provider-drift canary fixture (issue #265, spec
//! merge blocker 4). The frozen vectors were embedded under the `instr1`
//! WRITE-scheme text function (`embed_scheme::doc_text_for_entry`) so the
//! weekly cron can compare re-embeds "like with like". This test never touches
//! the network — it validates SHAPE and BINDING:
//!
//! - exactly 32 rows, one per canary text;
//! - every vector is 2560-dim (Qwen3 embedding-4b), non-zero;
//! - every text begins with the byte-exact `QUERY_INSTRUCTION_PREFIX`;
//! - every row's `model` is the model of the RAW gate key that owns the
//!   `instr1` floor in `WISDOM_GATE_FLOORS` (so a provider/model swap in the
//!   gate invalidates — rather than silently ignores — this canary);
//! - row ids are unique (the cron keys on them).

use serde_json::Value;

const CANARY_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/wisdom_gate_canary/canary.jsonl"
);

const CANARY_MODEL: &str = "qwen/qwen3-embedding-4b";
const CANARY_DIM: usize = 2560;
const CANARY_COUNT: usize = 32;

#[test]
fn wisdom_canary_fixture_shape() {
    let raw = std::fs::read_to_string(CANARY_PATH)
        .expect("canary.jsonl must be committed (spec blocker 4)");
    let rows: Vec<Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l)
                .unwrap_or_else(|e| panic!("canary.jsonl line is not valid JSON: {e}"))
        })
        .collect();

    assert_eq!(
        rows.len(),
        CANARY_COUNT,
        "canary fixture must hold exactly {CANARY_COUNT} rows"
    );

    let prefix = tauri_app_lib::embed_scheme::QUERY_INSTRUCTION_PREFIX;
    let instr1_key = tauri_app_lib::embed_scheme::WRITE_SCHEME;
    let raw_gate_key = tauri_app_lib::wisdom_match::WISDOM_GATE_FLOORS
        .iter()
        .map(|(k, _)| *k)
        // Test-only `stub:` keys carry no provider model (same exclusion as
        // the bench's snapshot-coverage check).
        .filter(|k| !k.starts_with("stub:"))
        .find(|k| k.ends_with(&format!(":{instr1_key}")))
        .map(|k| &k[..k.len() - instr1_key.len() - 1])
        .unwrap_or_else(|| panic!("WISDOM_GATE_FLOORS must contain a `:{instr1_key}` floor key"));
    assert_eq!(
        raw_gate_key,
        format!("external:{CANARY_MODEL}"),
        "canary model constant must track the RAW gate key that owns the instr1 floor"
    );

    let mut seen_ids = std::collections::HashSet::new();
    for (n, row) in rows.iter().enumerate() {
        let id = row["id"]
            .as_str()
            .unwrap_or_else(|| panic!("row {n}: missing id"));
        assert!(seen_ids.insert(id), "row {n}: duplicate canary id {id}");

        let model = row["model"]
            .as_str()
            .unwrap_or_else(|| panic!("row {n} ({id}): missing model"));
        assert_eq!(
            model, CANARY_MODEL,
            "row {n} ({id}): frozen vectors must be embedded with {CANARY_MODEL}"
        );

        let text = row["text"]
            .as_str()
            .unwrap_or_else(|| panic!("row {n} ({id}): missing text"));
        assert!(
            text.starts_with(prefix),
            "row {n} ({id}): text must start with the byte-exact QUERY_INSTRUCTION_PREFIX \
             (frozen vectors are valid only under the instr1 WRITE text function)"
        );

        let vector = row["vector"]
            .as_array()
            .unwrap_or_else(|| panic!("row {n} ({id}): missing vector"));
        assert_eq!(
            vector.len(),
            CANARY_DIM,
            "row {n} ({id}): vector must be {CANARY_DIM}-dim"
        );
        let finite = vector
            .iter()
            .all(|v| v.as_f64().is_some_and(f64::is_finite));
        let non_zero = vector.iter().any(|v| v.as_f64() != Some(0.0));
        assert!(
            finite && non_zero,
            "row {n} ({id}): vector must be finite and non-degenerate"
        );

        assert!(
            row["frozen_at"].as_str().is_some_and(|s| !s.is_empty()),
            "row {n} ({id}): missing frozen_at"
        );
        assert!(
            row["selection_rule"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "row {n} ({id}): missing selection_rule"
        );
    }
}
