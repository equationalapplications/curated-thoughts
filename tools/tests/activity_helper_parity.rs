//! `run_message_path` helper contract (issue #282 §8 step 4 — rev-13 M2 /
//! rev-14 m2): the extracted message-path executor RETURNS the `WisdomMatch`
//! it printed (plus the `Option<TwoStageOutcome>`), so a test can compare
//! the returned value against the direct library call with byte-identical
//! inputs. The helper never embeds and never opens the brain — the caller
//! owns both (rev-14 m2: `conn`, key, scheme, `max`, `exclude`, `now_ms`,
//! the two vectors, the stage flag and `json_mode` go IN).

mod common;

use common::with_seeded_brain;
use rusqlite::params;
use tauri_app_lib::embed_scheme::{self, Scheme};
use tauri_app_lib::retrieval;
use tauri_app_lib::wisdom_match as wm;

fn brain_db() -> rusqlite::Connection {
    let dir = std::env::var("CURATED_BRAIN_DIR").expect("with_seeded_brain sets it");
    rusqlite::Connection::open(std::path::Path::new(&dir).join("brain.db")).unwrap()
}

fn seed_fact(id: &str) {
    let mut v = [0f32; 8];
    v[0] = 1.0;
    let blob: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
    brain_db()
        .execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, 'ent_t', ?2, ?3, '[]', 'inferred', 'librarian_inferred',
                       NULL, NULL, 100, 100, NULL, 0, NULL, ?4, NULL)",
            params![id, format!("Title {id}"), format!("Body {id}"), blob],
        )
        .unwrap();
}

fn query_vec(text: &str) -> Vec<f32> {
    let brain = retrieval::resolve_brain_paths();
    let profile = retrieval::load_embed_profile(&brain.config_path).unwrap();
    tauri_app_lib::embedder::embed_one(&profile, text.to_string()).unwrap()
}

#[test]
fn helper_v1_arm_equals_the_direct_call_on_identical_inputs() {
    with_seeded_brain(|| {
        seed_fact("fact_a");
        let conn = brain_db();
        let brain = retrieval::resolve_brain_paths();
        let profile = retrieval::load_embed_profile(&brain.config_path).unwrap();
        let read_scheme = embed_scheme::read_scheme_for_reader(&conn).unwrap();
        let key = wm::gate_model_key(&profile, Some("constant8"));
        let schemed = query_vec("how do I deploy");
        let now = 1_000_i64;

        let (helper, outcome) = curated_thoughts_tools::queries::run_message_path(
            &conn,
            &key,
            read_scheme,
            &[], // raw vector: unused on the v1 arm
            &schemed,
            2,
            &[],
            now,
            false, // v1 arm
            true,  // json (printing goes to test stdout; the return is the contract)
        )
        .unwrap();
        assert!(outcome.is_none(), "v1 arm carries no two-stage outcome");

        let direct =
            wm::wisdom_match_in_scheme(&conn, &schemed, &key, read_scheme, 2, &[], now).unwrap();
        assert_eq!(
            helper, direct,
            "the helper's returned match must equal the direct call on the same inputs"
        );
        assert!(
            direct.entries.iter().any(|e| e.id == "fact_a"),
            "sanity: the stub vector clears the stub floor and delivers the fact"
        );
    });
}

#[test]
fn helper_two_stage_arm_equals_the_direct_call_on_identical_inputs() {
    with_seeded_brain(|| {
        seed_fact("fact_a");
        let conn = brain_db();
        let brain = retrieval::resolve_brain_paths();
        let profile = retrieval::load_embed_profile(&brain.config_path).unwrap();
        let read_scheme = embed_scheme::read_scheme_for_reader(&conn).unwrap();
        let key = wm::gate_model_key(&profile, Some("constant8"));
        // Raw scheme: the schemed vector IS the raw vector (rev-10 M1 reuse
        // pin — `Scheme::Raw => raw.clone()`).
        let raw = query_vec("how do I deploy");
        let schemed = raw.clone();
        let now = 1_000_i64;

        let (helper, outcome) = curated_thoughts_tools::queries::run_message_path(
            &conn,
            &key,
            read_scheme,
            &raw,
            &schemed,
            2,
            &[],
            now,
            true,
            true,
        )
        .unwrap();

        let direct = wm::wisdom_match_two_stage_with_v1(
            &conn,
            &raw,
            &schemed,
            read_scheme,
            2,
            &[],
            now,
            &key,
            wm::QueryKind::Message,
        )
        .unwrap();
        assert_eq!(helper, direct.result);
        assert_eq!(outcome.as_ref().map(|o| o.rule), Some(direct.outcome.rule));
        assert_eq!(outcome.as_ref().map(|o| o.open), Some(direct.outcome.open));
    });
}

/// Scheme-shape guard: under `raw` the schemed vector is the raw vector —
/// the reuse assumption the activity fallback leans on (rev-10 M1).
#[test]
fn raw_scheme_query_text_is_the_bare_text() {
    let text = "src/db/migrations/0042_add_users.sql";
    assert_eq!(
        embed_scheme::query_text_for_scheme(text, Scheme::Raw),
        text,
        "raw embeds the bare text verbatim — activity fallback vectors are identical"
    );
}
