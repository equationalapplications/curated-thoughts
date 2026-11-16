//! Task 7 E2E (spec 2026-11-14-areas §8): the outbox PAYLOAD half of the
//! areas carrier. INTEGRATION ONLY — every assertion here goes through the
//! PUB commit path (`resolve_proposal`) or the pub bundle-import + outbox
//! worker surfaces; the payload-SHAPE unit assertion lives in `commit.rs`
//! `mod tests` (the builders are `pub(crate)`, tier-3 r2 m2).
//!
//! Covers §8.5: outbox table rows carry `area` in their payload JSON; a
//! round trip through `sync_batch` passes the payload through unchanged; the
//! sink receives `OutboxEvent.payload` containing `area`. §8.4: bundle
//! import prints ONE loud per-import summary line and its fact payloads
//! carry `area: null` (bundles carry no area).

use rusqlite::params;
use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::db::commit::{resolve_proposal, ResolveOptions};
use tauri_app_lib::db::connection::{open_app_db, open_in_memory};
use tauri_app_lib::db::outbox_format::LLM_WIKI_OUTBOX_TABLE;
use tauri_app_lib::db::proposals::{
    insert_proposal, ItemDecision, ItemDecisionKind, NewProposal, NewProposalItem,
    NewProposalSource, ProposalKind, ProposalSourceRole, StoredEvidenceChunk,
};
use tauri_app_lib::db::queries::{insert_chunk, upsert_document};
use tauri_app_lib::okf::bundle_read::{ParsedBundle, ParsedEntity};
use tauri_app_lib::okf::types::{LLM_WIKI_PROFILE_V2, WikiFact};
use tauri_app_lib::db::bundle_apply::{apply_import, ImportMode};
use tauri_app_lib::outbox::{OutboxConfig, OutboxEvent, Sink};
use std::sync::{Arc, Mutex};

fn seed_entity(conn: &rusqlite::Connection, id: &str) {
    conn.execute(
        "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
         VALUES (?1, 'Test Entity', 'concept', 'summary', 100, 100)",
        params![id],
    )
    .unwrap();
}

/// Seeds a document + one live chunk; returns (doc_id, chunk_id).
fn seed_doc_and_chunk(conn: &rusqlite::Connection, path: &str) -> (i64, i64) {
    let doc_id = upsert_document(conn, path, "hash").unwrap();
    let chunk = Chunk {
        text: "evidence".into(),
        start_line: 1,
        end_line: 2,
        symbol_name: None,
        defined_symbol: None,
        strategy: ChunkStrategyTag::Prose,
    };
    let chunk_id = insert_chunk(conn, doc_id, &chunk, 0, "tier_fact", "").unwrap();
    (doc_id, chunk_id)
}

/// NewEntity proposal with one anchored fact_add carrying `area` in its
/// payload — the same shape the librarian synthesis validate pass emits.
fn insert_area_proposal(conn: &rusqlite::Connection, id: &str, doc_id: i64, chunk_id: i64) {
    insert_proposal(
        conn,
        &NewProposal {
            id: id.into(),
            kind: ProposalKind::NewEntity,
            entity_id: None,
            proposed_name: Some("Project X".into()),
            proposed_type: Some("project".into()),
            reasoning: Some("Because.".into()),
            model: "test".into(),
        },
        &[NewProposalItem {
            id: format!("item-{id}"),
            item_type: "fact_add".into(),
            target_id: None,
            payload: serde_json::json!({
                "body": "A scoped fact worth storing.",
                "tags": [],
                "confidence": "inferred",
                "area": "deploy"
            }),
            evidence: vec![StoredEvidenceChunk {
                chunk_id: Some(chunk_id),
                content_hash: String::new(),
                quote: "evidence".into(),
                start_line: Some(1),
                end_line: Some(2),
                source_kind: None,
            }],
        }],
        &[NewProposalSource {
            doc_id,
            role: ProposalSourceRole::Trigger,
        }],
    )
    .unwrap();
}

fn accept_sole_decision(conn: &rusqlite::Connection, proposal_id: &str) -> Vec<ItemDecision> {
    let item_id: String = conn
        .query_row(
            "SELECT id FROM curated_proposal_items WHERE proposal_id = ?1 LIMIT 1",
            params![proposal_id],
            |r| r.get(0),
        )
        .unwrap();
    vec![ItemDecision {
        item_id,
        decision: ItemDecisionKind::Accept,
        edited_payload: None,
    }]
}

/// Reads the entries-row outbox payload for `record_id` as parsed JSON.
fn outbox_payload(conn: &rusqlite::Connection, record_id: &str) -> serde_json::Value {
    let raw: String = conn
        .query_row(
            &format!(
                "SELECT payload FROM {LLM_WIKI_OUTBOX_TABLE}
                 WHERE table_name = 'entries' AND record_id = ?1"
            ),
            params![record_id],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}

/// §8.2 + §8.5, commit-path half: resolving a proposal whose fact_add payload
/// carries `area` writes an `llm_wiki_outbox` row whose payload JSON carries
/// the SAME area — column and payload can never disagree.
#[test]
fn outbox_payload_carries_area_via_pub_commit_path() {
    let mut conn = open_in_memory().unwrap();
    seed_entity(&conn, "ent-ob1");
    let (doc_id, chunk_id) = seed_doc_and_chunk(&conn, "/vault/documents/ob.pdf");
    insert_area_proposal(&conn, "prop-ob1", doc_id, chunk_id);

    let decisions = accept_sole_decision(&conn, "prop-ob1");
    let result = resolve_proposal(
        &mut conn,
        "prop-ob1",
        &decisions,
        None,
        ResolveOptions {
            auto_approve: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.committed.len(), 1, "fact must commit");
    let record_id = result.committed[0].record_id.clone();

    // Column and payload agree.
    let column_area: Option<String> = conn
        .query_row(
            "SELECT area FROM llm_wiki_entries WHERE id = ?1",
            params![record_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(column_area.as_deref(), Some("deploy"));
    let payload = outbox_payload(&conn, &record_id);
    assert_eq!(
        payload["area"],
        serde_json::json!("deploy"),
        "outbox payload must carry the area verbatim, got: {payload}"
    );
}

/// Test sink: collects every `OutboxEvent.payload` it receives (§8.5 sink
/// surface, `Sink::insert_event`).
#[derive(Clone, Default)]
struct CollectingSink(Arc<Mutex<Vec<serde_json::Value>>>);

impl Sink for CollectingSink {
    async fn insert_event(&self, event: &OutboxEvent) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(event.payload.clone());
        Ok(())
    }
}

/// §8.5, sync half, on a real on-disk brain DB: the payload the pub commit
/// path staged survives the `sync_batch` round trip unchanged and the sink
/// receives `OutboxEvent.payload` containing `area`.
#[test]
fn sync_batch_round_trip_delivers_area_payload_to_sink() {
    let tmp = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    let db_path = tmp.path().join("brain.db");
    let mut conn = open_app_db(&db_path, None).unwrap();
    seed_entity(&conn, "ent-ob2");
    let (doc_id, chunk_id) = seed_doc_and_chunk(&conn, "/vault/documents/ob2.pdf");
    insert_area_proposal(&conn, "prop-ob2", doc_id, chunk_id);

    let decisions = accept_sole_decision(&conn, "prop-ob2");
    let result = resolve_proposal(&mut conn, "prop-ob2", &decisions, None, ResolveOptions {
            auto_approve: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.committed.len(), 1);
    let record_id = result.committed[0].record_id.clone();
    drop(conn);

    // Round trip: the worker drains `llm_wiki_outbox` and the sink receives
    // the staged payload verbatim.
    let worker = tauri_app_lib::outbox::OutboxWorker::open(&db_path).unwrap();
    let sink = CollectingSink::default();
    let config = OutboxConfig {
        sqlite_path: db_path.clone(),
        db_url: "unused://test".into(),
        outbox_table: LLM_WIKI_OUTBOX_TABLE.to_string(),
        batch_size: 100,
        ..OutboxConfig::default()
    };
    let full = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(worker.sync_batch(&sink, &config))
        .unwrap();
    assert!(!full, "one event must not fill the batch");

    let received = sink.0.lock().unwrap();
    assert_eq!(received.len(), 1, "sink must receive exactly one event");
    assert_eq!(
        received[0]["area"],
        serde_json::json!("deploy"),
        "sink payload must carry the area unchanged, got: {}",
        received[0]
    );
    assert_eq!(received[0]["id"], serde_json::json!(record_id));
}

/// Bundle fixture mirroring `bundle_apply.rs mod tests::sample_fact`.
fn bundle_fact(id: &str, entity_id: &str) -> WikiFact {
    WikiFact {
        id: id.into(),
        entity_id: entity_id.into(),
        title: "A fact".into(),
        body: "Body.".into(),
        tags: vec![],
        confidence: "certain".into(),
        source_type: "user_confirmed".into(),
        source_hash: None,
        source_ref: None,
        created_at: 1719835200000,
        updated_at: 1719835200000,
        last_accessed_at: None,
        access_count: 0,
        deleted_at: None,
        okf_type: None,
        lifecycle_status: "stable".into(),
        stale_after: None,
        generated_by: None,
        okf_sources: None,
        okf_verified: None,
        okf_usage_window: None,
        last_verified_at: None,
        last_verified_by: None,
        evidence_json: None,
    }
}

/// §8.4, payload half: bundle-applied facts carry `area: null` in their
/// outbox payload — bundles never carry area; the row lands whole-repo.
#[test]
fn bundle_import_fact_payloads_carry_null_area() {
    let mut conn = open_in_memory().unwrap();
    let bundle = ParsedBundle {
        profile: Some(LLM_WIKI_PROFILE_V2.into()),
        entities: vec![ParsedEntity {
            entity_id: "ent_b".into(),
            display_name: Some("Project B".into()),
            summary: Some("Bundle summary.".into()),
            facts: vec![bundle_fact("fact_b1", "ent_b")],
            ..ParsedEntity::default()
        }],
        ..ParsedBundle::default()
    };
    let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();
    assert_eq!(result.facts_added, 1);

    let payload = outbox_payload(&conn, "fact_b1");
    assert!(
        payload["area"].is_null(),
        "bundle fact payload must carry area: null, got: {payload}"
    );
}
