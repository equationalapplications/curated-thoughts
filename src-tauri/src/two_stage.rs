//! Stage-1 closed-set chunk search for the wisdom gate (spec §3.1, §4 GUARD, §6).
//!
//! Task 3 of docs/superpowers/plans/2026-10-15-issue271-two-stage-retrieval.md.
//! Everything here is the CLOSED-SET leg: the candidate doc population, the
//! structural complement, the scored top-k, membership, and coverage probes.
//! The v1-merge decision itself is Task 4 (wisdom_match.rs). Schema-skewed or
//! unprobeable databases degrade to `None`/empty — the caller takes the §4 v1
//! fallback; this module never errors on probe misses.
use anyhow::Result;
use rusqlite::Connection;
use rusqlite::OptionalExtension;

use crate::db::ddl_compat::existing_columns;
use crate::wisdom_match::{eligible_entry_ids, entry_eligibility_where, temporal_columns};

/// One stage-1 candidate population. `doc_ids` = evidence-hash hop ∪ proposal
/// chain — the raw UNION: doc-level dedupe is FORBIDDEN (r29-M2: membership
/// is by doc_id; dedupe applies ONLY at result level in [`stage1_search`]).
pub struct CandidateSet {
    pub doc_ids: Vec<i64>,
}

/// One scored stage-1 chunk (result-level dedupe ALREADY applied: duplicate
/// identical vectors collapse — same text ⇒ same vector ⇒ keep one; NO
/// row-level doc dedupe, r29-M2).
pub struct Stage1Hit {
    pub chunk_id: i64,
    pub doc_id: i64,
    pub score: f32,
}

/// BOTH probes pinned (spec §6 r28-M3 + r30-m2 + r29-m5): `documents` columns
/// (embed_key AND last_indexed_hash) plus table existence for the hop legs
/// (librarian_evidence, curated_proposal_sources, chunks). Any miss ⇒
/// Ok(false) ⇒ caller takes the §4 v1 fallback. Stage-1 SQL runs ONLY after
/// the hop tables are confirmed present.
pub fn two_stage_schema_ready(conn: &Connection) -> Result<bool> {
    for table in ["librarian_evidence", "curated_proposal_sources", "chunks"] {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |r| r.get(0),
        )?;
        if n == 0 {
            return Ok(false);
        }
    }
    let cols = existing_columns(conn, "documents")?;
    Ok(cols.iter().any(|c| c == "embed_key") && cols.iter().any(|c| c == "last_indexed_hash"))
}

/// Evidence-hash hop ∪ proposal chain (r28-M1 — the REAL edges; there is no
/// `source_hash` column on curated_proposal_sources and `documents.hash` is
/// NOT a membership predicate). A dangling proposal leg JOINS TO NOTHING and
/// lands in the complement — never lean on CASCADE (CT connections never set
/// PRAGMA foreign_keys; librarian_evidence.proposal_id has no FK).
///
/// `{elig}` is the shared Task-2 eligibility fragment (alias `e` =
/// `llm_wiki_entries`), carrying the temporal AND the V27 read-scheme filter,
/// so a candidate is reachable only through an ELIGIBLE entry — the gate and
/// stage-1 can never disagree about which facts anchor an edge. The outer
/// SELECT keeps the raw UNION of both legs (doc_ids MAY repeat; r29-M2 —
/// result-level dedupe lives in stage1_search only). `:now` is bound iff the
/// fragment references it (the temporal arms are its only `:now` consumers).
const CANDIDATE_DOCS_SQL: &str = "
SELECT doc_id FROM (
  SELECT DISTINCT c.doc_id AS doc_id
    FROM librarian_evidence le
    JOIN llm_wiki_entries e ON e.id = le.entry_id
    JOIN json_each(le.evidence_json, '$.evidence') je
    JOIN chunks c ON c.content_hash = json_extract(je.value, '$.content_hash')
    WHERE {elig}
  UNION
  SELECT DISTINCT ps.doc_id AS doc_id
    FROM librarian_evidence le
    JOIN llm_wiki_entries e ON e.id = le.entry_id
    JOIN curated_proposal_sources ps ON ps.proposal_id = le.proposal_id
    WHERE {elig}
)";

pub fn candidate_set(
    conn: &Connection,
    now_ms: i64,
    scheme: Option<crate::embed_scheme::Scheme>,
) -> Result<Option<CandidateSet>> {
    if !two_stage_schema_ready(conn)? {
        return Ok(None);
    }
    let t = temporal_columns(conn, scheme)?;
    let elig = entry_eligibility_where(&t);
    let sql = CANDIDATE_DOCS_SQL.replacen("{elig}", &elig, 2);
    let mut stmt = conn.prepare(&sql)?;
    // The temporal arms are the fragment's ONLY `:now` consumers, so the
    // fragment text itself tells whether a binding is needed (Temporal's
    // fields are private by design — the fragment is the contract).
    let bind_now = elig.contains(":now");
    let map = |r: &rusqlite::Row<'_>| r.get::<_, i64>(0);
    let doc_ids: Vec<i64> = if bind_now {
        stmt.query_map(rusqlite::named_params! { ":now": now_ms }, map)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map([], map)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(Some(CandidateSet { doc_ids }))
}

/// Entries reachable from the candidate set under `rule` (structural, spec
/// §3 v1-merge). Rules: `"restricted-cosine"` — rule (i), reachable iff the
/// entry is carried by a candidate doc with >=1 stage-1-scorable chunk;
/// `"chunk-hop"` — rule (ii), reachable only through a member chunk (see
/// [`member_chunk_ids`]). The proposal-chain leg WITHOUT a member chunk is
/// rule-(ii)-dark (r29-M1); chunk-id-only evidence counts as COMPLEMENT
/// (spec §3 class 5, r30-m4 scoping).
fn reachable_entry_ids(
    conn: &Connection,
    cands: &CandidateSet,
    rule: &str,
    now_ms: i64,
    scheme: Option<crate::embed_scheme::Scheme>,
) -> Result<Vec<String>> {
    if cands.doc_ids.is_empty() {
        return Ok(Vec::new());
    }
    let t = temporal_columns(conn, scheme)?;
    let elig = entry_eligibility_where(&t);
    let doc_list = cands
        .doc_ids
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let leg_sql = match rule {
        // Rule (i): the hash-hop leg reaches through its OWN chunk's
        // scorability; the proposal leg through ANY scorable chunk on the
        // doc (EXISTS over chunks⋈embeddings).
        "restricted-cosine" => format!(
            "SELECT DISTINCT le.entry_id AS id
               FROM librarian_evidence le
               JOIN json_each(le.evidence_json, '$.evidence') je
               JOIN chunks c ON c.content_hash = json_extract(je.value, '$.content_hash')
               JOIN embeddings emb ON emb.chunk_id = c.id
              WHERE c.doc_id IN ({doc_list})
              UNION
             SELECT DISTINCT le.entry_id AS id
               FROM librarian_evidence le
               JOIN curated_proposal_sources ps ON ps.proposal_id = le.proposal_id
              WHERE ps.doc_id IN ({doc_list})
                AND EXISTS (SELECT 1 FROM chunks c
                              JOIN embeddings emb ON emb.chunk_id = c.id
                             WHERE c.doc_id = ps.doc_id)"
        ),
        // Rule (ii): member chunks only — hash-hop hit AND scorable; a
        // proposal-chain-only doc is outside (ii)'s reach ⇒ complement.
        "chunk-hop" => format!(
            "SELECT DISTINCT le.entry_id AS id
               FROM librarian_evidence le
               JOIN json_each(le.evidence_json, '$.evidence') je
               JOIN chunks c ON c.content_hash = json_extract(je.value, '$.content_hash')
               JOIN embeddings emb ON emb.chunk_id = c.id
              WHERE c.doc_id IN ({doc_list})"
        ),
        other => anyhow::bail!("two_stage: unknown complement rule {other:?}"),
    };
    // Re-run the LIVE eligibility predicate over the reachable ids — the
    // same fragment, the same `:now` binding, so the complement can never
    // include an ineligible entry nor keep an eligible one out.
    let sql = format!(
        "SELECT DISTINCT e.id FROM llm_wiki_entries e \
         WHERE {elig} AND e.id IN ({leg_sql})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let bind_now = elig.contains(":now");
    let rows: Vec<String> = if bind_now {
        stmt.query_map(
            rusqlite::named_params! { ":now": now_ms },
            |r| r.get::<_, String>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}

pub fn complement_entry_ids(
    conn: &Connection,
    cands: &CandidateSet,
    rule: &str,
    now_ms: i64,
    scheme: Option<crate::embed_scheme::Scheme>,
) -> Result<Vec<String>> {
    let t = temporal_columns(conn, scheme)?;
    let eligible = eligible_entry_ids(conn, &t, now_ms)?;
    let reachable = reachable_entry_ids(conn, cands, rule, now_ms, scheme)?;
    let reachable: std::collections::HashSet<String> = reachable.into_iter().collect();
    // The complement computed STRUCTURALLY (eligible set anti-join the
    // reachable set) — never by provenance (r27-M1/r29-M1).
    Ok(eligible
        .into_iter()
        .filter(|id| !reachable.contains(id))
        .collect())
}

pub fn stage1_search(
    conn: &Connection,
    query_vec: &[f32],
    cands: &CandidateSet,
    k: usize,
) -> Result<Vec<Stage1Hit>> {
    if cands.doc_ids.is_empty() {
        return Ok(Vec::new());
    }
    let id_list = cands
        .doc_ids
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT c.id, c.doc_id, emb.vector FROM chunks c \
         JOIN embeddings emb ON emb.chunk_id = c.id \
         WHERE c.doc_id IN ({id_list})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut scored: Vec<Stage1Hit> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|(chunk_id, doc_id, bytes)| {
            let v = crate::search::bytes_to_f32(&bytes);
            Stage1Hit {
                chunk_id,
                doc_id,
                score: crate::search::cosine_similarity(query_vec, &v),
            }
        })
        .collect();
    // r29-M2: dedupe at RESULT level only — duplicate identical VECTORS keep
    // one (same text ⇒ same stored vector). Byte equality via `same_vector`
    // decides; a bare score tie is a genuine near-duplicate and stays. Keep
    // the lowest chunk_id for determinism; truncate to k AFTER dedupe.
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.chunk_id.cmp(&b.chunk_id))
    });
    let mut out: Vec<Stage1Hit> = Vec::new();
    for h in scored {
        let dup = match out.last() {
            // Byte-equal vectors always tie numerically, so checking the
            // current score-group head is sufficient.
            Some(p) => {
                (p.score - h.score).abs() < 1e-6 && same_vector(conn, p.chunk_id, h.chunk_id)?
            }
            None => false,
        };
        if !dup {
            out.push(h);
        }
    }
    out.truncate(k);
    Ok(out)
}

/// Exact byte equality of two embeddings rows' vectors — identical text ⇒
/// identical stored vector.
fn same_vector(conn: &Connection, a: i64, b: i64) -> Result<bool> {
    if a == b {
        return Ok(true);
    }
    let va: Option<Vec<u8>> = conn
        .query_row(
            "SELECT vector FROM embeddings WHERE chunk_id = ?1",
            [a],
            |r| r.get(0),
        )
        .optional()?;
    let vb: Option<Vec<u8>> = conn
        .query_row(
            "SELECT vector FROM embeddings WHERE chunk_id = ?1",
            [b],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match (va, vb) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    })
}

/// Chunk-level membership for rule (ii): chunk ids of candidate docs whose
/// content_hash appears in a live (non-deleted entry's) librarian_evidence
/// evidence blob AND that are stage-1-scorable (>=1 embeddings row).
pub fn member_chunk_ids(conn: &Connection, cands: &CandidateSet) -> Result<Vec<i64>> {
    if cands.doc_ids.is_empty() {
        return Ok(Vec::new());
    }
    let id_list = cands
        .doc_ids
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT DISTINCT c.id FROM chunks c \
         WHERE c.doc_id IN ({id_list}) \
           AND c.id IN (SELECT emb.chunk_id FROM embeddings emb) \
           AND c.content_hash IN ( \
             SELECT json_extract(je.value, '$.content_hash') \
               FROM librarian_evidence le \
               JOIN llm_wiki_entries e2 ON e2.id = le.entry_id AND e2.deleted_at IS NULL \
               JOIN json_each(le.evidence_json, '$.evidence') je \
              WHERE json_extract(je.value, '$.content_hash') IS NOT NULL)"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// §6 coverage over candidate docs with >=1 stage-1-scorable chunk, BEFORE
/// the per-call exclude/correction skip set (never call-dependent). Returns
/// (candidate_doc_count, covered_count); `Ok(None)` = probe failed ⇒ v1
/// fallback. `now_ms` feeds only the eligibility predicate (plan NOTE at the
/// Task-3 tests: never call `candidate_set(conn, 0, None)`).
pub fn coverage(conn: &Connection, profile_key: &str, now_ms: i64) -> Result<Option<(i64, i64)>> {
    if !two_stage_schema_ready(conn)? {
        return Ok(None);
    }
    let cands = match candidate_set(conn, now_ms, None)? {
        Some(c) => c,
        None => return Ok(None),
    };
    if cands.doc_ids.is_empty() {
        return Ok(Some((0, 0))); // vacuously covered
    }
    // Population computed BEFORE the embed_key predicate (r26-M2): n counts
    // scorable candidate docs; covered counts those whose embed_key matches.
    let id_list = cands
        .doc_ids
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN d.embed_key = ?1 THEN 1 ELSE 0 END), 0) \
           FROM documents d \
          WHERE d.id IN ({id_list}) \
            AND EXISTS (SELECT 1 FROM chunks c \
                          JOIN embeddings emb ON emb.chunk_id = c.id \
                         WHERE c.doc_id = d.id)"
    );
    let (n, cov) = conn.query_row(&sql, [profile_key], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
    })?;
    Ok(Some((n, cov)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::embed_scheme::Scheme;
    use crate::wisdom_match::wisdom_match_with_floor;

    const NOW_MS: i64 = 1_760_000_000_000;

    fn blob(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Orthonormal 4-dim basis vector — cosine against another basis vector
    /// is exactly 0.0 or 1.0, so score assertions are exact.
    fn unit_vec(i: i64) -> Vec<f32> {
        let mut v = vec![0.0_f32; 4];
        v[(i as usize - 1) % 4] = 1.0;
        v
    }

    fn fixture() -> Connection {
        let conn = open_in_memory().unwrap();
        // docs 1,2 candidate (1 via evidence hash hop, 2 via proposal chain);
        // doc 3 NOT candidate; doc 4 candidate-shaped but reached by NO edge.
        for (id, key) in [
            (1, "stub:constant8"),
            (2, "stub:constant8"),
            (3, "old-model"),
            (4, ""),
        ] {
            conn.execute(
                "INSERT INTO documents (path, hash, status, tier, embed_key)
                 VALUES (?1, ?1, 'indexed', 'user_doc', ?2)",
                rusqlite::params![
                    format!("/v/d{id}.md"),
                    if key.is_empty() { None } else { Some(key) }
                ],
            )
            .unwrap();
        }
        // chunks: doc1 c1,e1; doc2 c2,e1; doc3 c3,e1; doc4 has no chunks (r26-M2).
        // c1 and c2 carry IDENTICAL vectors (same text ⇒ same vector) so the
        // result-level dedupe test has a TRUE duplicate across two docs;
        // c3 stays embedding-less as the negative control.
        for (cid, did) in [(1, 1), (2, 2), (3, 3)] {
            conn.execute(
                "INSERT INTO chunks (doc_id, chunk_text, position, strategy)
                 VALUES (?1, 't', 0, 'prose')",
                [did],
            )
            .unwrap();
            if cid != 3 {
                let vec = if cid == 2 { unit_vec(1) } else { unit_vec(cid) };
                conn.execute(
                    "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
                    rusqlite::params![cid, blob(&vec)],
                )
                .unwrap();
            }
        }
        conn.execute_batch(
            "UPDATE chunks SET content_hash = 'h-c1' WHERE id = 1;
             INSERT INTO curated_proposals (id, kind, model, status, created_at)
               VALUES ('p1', 'new_entity', 'test', 'approved', 0);
             INSERT INTO curated_proposal_sources (proposal_id, doc_id, role)
               VALUES ('p1', 2, 'evidence');",
        )
        .unwrap();
        // Entries carry a non-null embedding_blob: the shared eligibility
        // predicate (Task 2) requires it, so without it NO entry is eligible
        // and every edge leg joins to nothing.
        for (id, entity, st, sref) in [
            ("f1", "e1", "librarian_inferred", "x"),
            ("f2", "e2", "user_stated", ""),
            ("f3", "e3", "librarian_inferred", "x"),
        ] {
            conn.execute(
                "INSERT INTO llm_wiki_entries
                   (id, entity_id, title, body, source_type, source_ref,
                    created_at, updated_at, embedding_blob)
                 VALUES (?1, ?2, 'T', 'B', ?3, ?4, 0, 0, ?5)",
                rusqlite::params![
                    id,
                    entity,
                    st,
                    if sref.is_empty() { None } else { Some(sref) },
                    blob(&unit_vec(4))
                ],
            )
            .unwrap();
        }
        conn.execute_batch(
            "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json, created_at) VALUES
               ('f1','p1','{\"evidence\":[{\"content_hash\":\"h-c1\"}]}', 0),
               ('f3','p1','{\"evidence\":[{\"content_hash\":\"h-c9-gone\"}]}', 0);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn candidate_set_is_union_of_both_edges_no_doc_dedupe() {
        let conn = fixture();
        let cs = candidate_set(&conn, NOW_MS, None).unwrap().unwrap();
        // doc1 (evidence hash hop) ∪ doc2 (proposal chain).
        assert!(cs.doc_ids.contains(&1) && cs.doc_ids.contains(&2));
        assert!(!cs.doc_ids.contains(&3), "non-candidate doc excluded");
    }

    #[test]
    fn stage1_search_dedupes_identical_vectors_at_result_level_only() {
        let conn = fixture();
        // c1 and c2 carry IDENTICAL vectors ⇒ one result even though two docs.
        let cs = candidate_set(&conn, NOW_MS, None).unwrap().unwrap();
        let hits = stage1_search(&conn, &unit_vec(1), &cs, 8).unwrap();
        let scores: Vec<f32> = hits.iter().map(|h| h.score).collect();
        let dupes = scores.iter().filter(|s| (**s - 1.0).abs() < 1e-6).count();
        assert_eq!(dupes, 1, "identical vectors collapse in top-k (r29-M2)");
    }

    #[test]
    fn complement_is_structural_per_rule() {
        let conn = fixture();
        let cs = candidate_set(&conn, NOW_MS, None).unwrap().unwrap();
        let comp_i = complement_entry_ids(&conn, &cs, "restricted-cosine", NOW_MS, None).unwrap();
        // f2 (user_stated, no edge) + f3 (evidence hash h-c9-gone resolves to
        // nothing live AND its proposal edge p1→doc2 IS a scorable candidate ⇒
        // f3 is rule-(i)-REACHABLE via the proposal chain, but rule-(ii)-dark
        // unless a member chunk exists).
        assert!(comp_i.contains(&"f2".to_string()));
        assert!(
            !comp_i.contains(&"f3".to_string()),
            "proposal edge ⇒ rule-(i) reachable"
        );
        let comp_ii = complement_entry_ids(&conn, &cs, "chunk-hop", NOW_MS, None).unwrap();
        assert!(comp_ii.contains(&"f2".to_string()));
        // doc2 (proposal-chain-only candidate, zero member chunks) ⇒ its facts
        // are outside rule (ii)'s reach ⇒ complement (r29-M1). f1 IS reachable
        // under (ii): chunk 1 is a member chunk (h-c1 in f1's evidence).
        assert!(!comp_ii.contains(&"f1".to_string()));
    }

    #[test]
    fn coverage_counts_only_scorable_candidate_docs_and_is_call_independent() {
        let conn = fixture();
        let (n, cov) = coverage(&conn, "stub:constant8", NOW_MS).unwrap().unwrap();
        // population: candidate docs with >=1 embeddings-row chunk = docs 1,2.
        assert_eq!((n, cov), (2, 2));
        assert_eq!(
            coverage(&conn, "old-model", NOW_MS).unwrap().unwrap(),
            (2, 0)
        );
    }

    #[test]
    fn probes_return_none_on_schema_skew() {
        let conn = Connection::open_in_memory().unwrap();
        // bare DB: no documents.embed_key column, no librarian_evidence table.
        conn.execute_batch("CREATE TABLE documents (id INTEGER PRIMARY KEY, path TEXT);")
            .unwrap();
        assert!(candidate_set(&conn, NOW_MS, None).unwrap().is_none());
        assert!(coverage(&conn, "k", NOW_MS).unwrap().is_none());
    }

    /// SOR Finding 1 (Task-2 review): pins the FLIPPED supersession comparison
    /// the shared predicate encodes (`superseded_at > :now` stays eligible —
    /// a scheduled-but-not-yet-effective supersession must NOT drop an entry)
    /// in BOTH consumers of the fragment: the standalone eligible set AND the
    /// v1 gate (`wisdom_match_with_floor`). Without this pin the Task-2 flip
    /// is behaviorally unpinned.
    #[test]
    fn future_supersession_stays_eligible() {
        let conn = fixture();
        conn.execute(
            "INSERT INTO llm_wiki_entries
               (id, entity_id, title, body, source_type, embedding_blob,
                created_at, updated_at, superseded_by, superseded_at)
             VALUES ('fs','e9','T9','B9','librarian_inferred', ?1, 0, 0, 'f-newer', ?2)",
            rusqlite::params![blob(&[1.0, 0.0]), NOW_MS + 1000],
        )
        .unwrap();

        // Consumer 1: the shared standalone eligible set.
        let temporal = crate::wisdom_match::temporal_columns(&conn, Some(Scheme::Raw)).unwrap();
        let ids = crate::wisdom_match::eligible_entry_ids(&conn, &temporal, NOW_MS).unwrap();
        assert!(
            ids.contains(&"fs".to_string()),
            "a supersession scheduled in the future must NOT yet exclude the entry"
        );

        // Consumer 2: the v1 gate — same entry must surface as a scored match
        // (cosine 1.0 against its own vector, above the stub floor 0.5).
        let m = wisdom_match_with_floor(
            &conn,
            &[1.0, 0.0],
            "stub:constant8",
            Scheme::Raw,
            Some(0.5),
            8,
            &[],
            NOW_MS,
        )
        .unwrap();
        assert!(
            m.entries.iter().any(|e| e.id == "fs"),
            "the v1 gate must still score an entry whose supersession is in the future"
        );
    }
}
