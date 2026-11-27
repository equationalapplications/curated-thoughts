//! Transactional OKF bundle import: preview + apply for merge/replace/clone.
//! Summary writes go to `curated_entities.summary` only (backend spec
//! addendum point 4, option (a)) — never to llm_wiki_meta.

use std::collections::HashMap;

use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::db::entity_gate::ImmediateTx;
use serde::{Deserialize, Serialize};

use crate::db::commit::{
    push_entries_outbox, push_tasks_outbox, wiki_fact_outbox_payload, wiki_task_outbox_payload,
};
use crate::db::edge_purge::{endpoint_is_live, purge_dead_edges, purge_edges_for_hard_deleted};
use crate::db::outbox_format::OutboxOperation;
use crate::okf::bundle_read::{ParsedBundle, ParsedEntity};
use crate::okf::ids::generate_id;
use crate::okf::timefmt::ms_from_utc_date;
#[cfg(test)]
use crate::okf::types::LLM_WIKI_PROFILE_V2;

/// Per-fact synthesized `okf_sources` JSON, keyed by fact id.
/// Populated by `synthesize_sources_from_body` when a profile-1 fact
/// has no `sources` key but its body carries a `# Citations` section.
type SyntheticSources = std::collections::HashMap<String, String>;

/// v0.1 → v0.2 fallback (upstream §4.8): synthesize `okf_sources` from a
/// `# Citations` body list when a profile-1 fact has no `sources` key.
/// Captures every URL, not just the first.
fn synthesize_sources_from_body(bundle: &ParsedBundle) -> SyntheticSources {
    let mut out = SyntheticSources::new();
    for entity in &bundle.entities {
        for fact in &entity.facts {
            if fact.okf_sources.is_some() {
                continue;
            }
            if !fact.body.contains("# Citations") {
                continue;
            }
            let urls = extract_citations_urls(&fact.body);
            if urls.is_empty() {
                continue;
            }
            let entries: Vec<serde_json::Value> = urls
                .into_iter()
                .map(|u| serde_json::json!({ "resource": u }))
                .collect();
            out.insert(
                fact.id.clone(),
                serde_json::to_string(&entries).unwrap_or_default(),
            );
        }
    }
    out
}

/// Byte index of the first ASCII `http` in `s`, case-insensitive.
/// Returns `None` for empty inputs or strings shorter than `http`.
/// Used by the v0.1 → v0.2 `# Citations` fallback so URLs whose protocol
/// is upper- or mixed-case (`HTTPS://…`, `Https://…`, etc.) are still captured.
fn find_http_ci(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.len() < 4 {
        return None;
    }
    (0..=bytes.len() - 4).find(|&i| {
        bytes[i].eq_ignore_ascii_case(&b'h')
            && bytes[i + 1].eq_ignore_ascii_case(&b't')
            && bytes[i + 2].eq_ignore_ascii_case(&b't')
            && bytes[i + 3].eq_ignore_ascii_case(&b'p')
    })
}

/// Scan a body for a `# Citations` section and collect every URL on subsequent lines.
fn extract_citations_urls(body: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut in_section = false;
    for line in body.lines() {
        if line.trim_start().starts_with("# Citations") {
            in_section = true;
            // Standard markdown puts a blank line between a heading and its
            // body — fall through to consume it instead of breaking out.
            continue;
        }
        if in_section {
            // A second `# …` heading ends the section. Anything else (blank
            // lines included) is part of the citation list.
            if line.trim_start().starts_with('#') && !line.trim().is_empty() {
                break;
            }
            let mut rest = line;
            while let Some(idx) = find_http_ci(rest) {
                let url_start = idx;
                let mut end = url_start;
                let bytes = rest.as_bytes();
                while end < bytes.len()
                    && !(bytes[end] as char).is_whitespace()
                    && bytes[end] != b')'
                    && bytes[end] != b']'
                {
                    end += 1;
                }
                let url = rest[url_start..end].to_string();
                // Reject prose matches such as `https headers are required`.
                // Both `http://…` and `https://…` start with `http`; the bytes
                // after position 4 are `://` or `s://` respectively (case-
                // insensitive — the leading `http` was matched case-
                // insensitively too). Anything else is not a valid URL.
                let tail_lower = url[4..].to_ascii_lowercase();
                let has_scheme = tail_lower.starts_with("://") || tail_lower.starts_with("s://");
                if url.len() > 4 && has_scheme {
                    urls.push(url);
                }
                rest = &rest[end..];
            }
        }
    }
    urls
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    Merge,
    Replace,
    Clone,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityImportPreview {
    pub entity_id: String,
    pub name: String,
    pub entity_exists: bool,
    pub facts_new: i64,
    pub facts_existing: i64,
    pub tasks_new: i64,
    pub tasks_existing: i64,
    pub edges_total: i64,
    pub events_new: i64,
    pub events_duplicate: i64,
    pub summary_action: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportPreview {
    pub profile: Option<String>,
    pub entities: Vec<EntityImportPreview>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportResult {
    pub entities_touched: i64,
    pub facts_added: i64,
    pub facts_skipped: i64,
    pub tasks_added: i64,
    pub tasks_skipped: i64,
    pub edges_added: i64,
    pub events_added: i64,
    pub events_skipped: i64,
    /// Non-fatal provenance losses (review round 5, findings 3+10): a
    /// librarian fact whose legacy source_ref could not be salvaged, or whose
    /// evidence blob has no usable `proposal_id`. The dropped data is
    /// preserved verbatim in the message so nothing is silently destroyed.
    pub warnings: Vec<String>,
}

fn now_timestamps() -> (i64, i64) {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (dur.as_secs() as i64, dur.as_millis() as i64)
}

fn row_exists(conn: &Connection, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<bool> {
    Ok(conn
        .query_row(sql, params, |_| Ok(()))
        .optional()?
        .is_some())
}

fn fact_exists(conn: &Connection, id: &str) -> Result<bool> {
    row_exists(conn, "SELECT 1 FROM llm_wiki_entries WHERE id=?1", &[&id])
}

/// The import's entity cluster (survivor ∪ redirect losers), resolved once
/// per entity and cached (T4 r1 m2, r4 m4). The dup tombstone and the
/// fresh-INSERT linkage gate both scope against this set.
fn entity_cluster(
    cache: &mut Option<Vec<String>>,
    conn: &Connection,
    survivor: &str,
) -> Result<Vec<String>> {
    if let Some(c) = cache {
        return Ok(c.clone());
    }
    let c = crate::db::entities::cluster_ids(conn, survivor)?;
    *cache = Some(c.clone());
    Ok(c)
}

/// True iff `source_ref` is the normative token shape `^librarian-[0-9a-f]{32}$`
/// (spec §2.2). Delegates to the canonical `commit::is_librarian_source_ref_token`
/// — review round 5, finding 2: this local copy accepted UPPERCASE hex via
/// `is_ascii_hexdigit`, so a human-edited bundle carrying `librarian-<32
/// uppercase hex>` was stored verbatim while every reader (grounding, source
/// docs, census) classified it as a legacy document path — provenance resolved
/// to nothing and the next heal sweep soft-deleted the healthy imported fact.
fn is_librarian_token_shaped(source_ref: &str) -> bool {
    crate::db::commit::is_librarian_source_ref_token(source_ref)
}

fn task_exists(conn: &Connection, id: &str) -> Result<bool> {
    row_exists(conn, "SELECT 1 FROM llm_wiki_tasks WHERE id=?1", &[&id])
}

fn event_exists(
    conn: &Connection,
    entity_id: &str,
    event_id: Option<&str>,
    event_type: &str,
    summary: &str,
    date: &str,
) -> Result<bool> {
    if let Some(id) = event_id {
        if row_exists(conn, "SELECT 1 FROM llm_wiki_events WHERE id=?1", &[&id])? {
            return Ok(true);
        }
        return Ok(false);
    }
    row_exists(
        conn,
        "SELECT 1 FROM llm_wiki_events
         WHERE entity_id=?1 AND event_type=?2 AND summary=?3
           AND date(created_at/1000, 'unixepoch')=?4",
        &[&entity_id, &event_type, &summary, &date],
    )
}

pub fn preview_import(
    conn: &Connection,
    bundle: &ParsedBundle,
    mode: ImportMode,
) -> Result<ImportPreview> {
    let mut entities = Vec::new();
    let mut warnings = bundle.warnings.clone();
    // The apply mints every NEW entity through the §2.5 production gate and
    // aborts atomically on a Held mint; preview resolves the same ladder
    // (read-only) so it never reports an import the apply will refuse.
    let policy = crate::config::ingest_policy_for_db(conn.path());
    // Held new entities grouped by cause, in first-seen order, so the
    // warning names each cause once (a whole bundle usually shares one).
    let mut held: Vec<(crate::db::entity_gate::HoldReason, Vec<String>)> = Vec::new();
    for entity in &bundle.entities {
        // Task 7: preview resolves bundle source ids through redirects the
        // same way apply does, so a merged-away id previews as its survivor.
        let resolved = crate::db::entities::resolve_entity_id(conn, &entity.entity_id)?;
        let entity_exists = row_exists(
            conn,
            "SELECT 1 FROM curated_entities WHERE id=?1",
            &[&resolved],
        )?;
        // Probe the gate exactly as apply will mint (review finding): Clone
        // mode ALWAYS mints a fresh `ent_` id and gates THAT id — whether
        // or not the bundle id exists locally — so the probe uses a fresh
        // id too (the gate consults per-id state: rung 1a opt-outs,
        // entity-scoped manifests). Merge/Replace mint under the resolved
        // bundle id, only when it does not exist yet.
        let (probe_id, mints_new) = if mode == ImportMode::Clone {
            (generate_id("ent_"), true)
        } else {
            (resolved.clone(), !entity_exists)
        };
        if mints_new {
            let decision =
                crate::db::entity_gate::preview_production_gate(conn, &policy, &probe_id, &[]);
            // The apply's Held set, not just `GateDecision::Held`: the
            // shared insert helper ALSO holds a `Gate` decision over an
            // unusable vocabulary (`NodeVocabulary::hold_reason`). Preview
            // must refuse exactly what the apply would abort on, else it
            // reports an import the apply refuses (bundle mints are
            // unlabeled, so no other ladder arm can turn Held inside a
            // usable `Gate` decision).
            let hold_reason = match decision {
                crate::db::entity_gate::GateDecision::Held(reason) => Some(reason),
                crate::db::entity_gate::GateDecision::Gate(v) => v.hold_reason(None),
                crate::db::entity_gate::GateDecision::Skip => None,
            };
            if let Some(reason) = hold_reason {
                match held.iter_mut().find(|(r, _)| *r == reason) {
                    Some((_, ids)) => ids.push(entity.entity_id.clone()),
                    None => held.push((reason, vec![entity.entity_id.clone()])),
                }
            }
        }
        let local_summary: Option<String> = conn
            .query_row(
                "SELECT summary FROM curated_entities WHERE id=?1",
                [&resolved],
                |r| r.get(0),
            )
            .optional()?;

        let (mut facts_new, mut facts_existing) = (0i64, 0i64);
        for fact in &entity.facts {
            if mode != ImportMode::Clone && fact_exists(conn, &fact.id)? {
                facts_existing += 1;
            } else {
                facts_new += 1;
            }
        }
        let (mut tasks_new, mut tasks_existing) = (0i64, 0i64);
        for task in &entity.tasks {
            if mode != ImportMode::Clone && task_exists(conn, &task.id)? {
                tasks_existing += 1;
            } else {
                tasks_new += 1;
            }
        }
        let (mut events_new, mut events_duplicate) = (0i64, 0i64);
        for ev in &entity.events {
            let dup = mode != ImportMode::Clone
                && event_exists(
                    conn,
                    &entity.entity_id,
                    ev.event_id.as_deref(),
                    &ev.event_type,
                    &ev.summary,
                    &ev.date,
                )?;
            if dup {
                events_duplicate += 1;
            } else {
                events_new += 1;
            }
        }

        let summary_action = match mode {
            ImportMode::Clone => "new",
            ImportMode::Replace if entity.summary.is_some() => "overwrite",
            ImportMode::Replace => "none",
            ImportMode::Merge => match (&entity.summary, local_summary.as_deref()) {
                (None, _) => "none",
                (Some(_), Some(local)) if !local.trim().is_empty() => "keep_local",
                (Some(_), _) => "fill",
            },
        }
        .to_string();

        entities.push(EntityImportPreview {
            entity_id: entity.entity_id.clone(),
            name: entity
                .display_name
                .clone()
                .unwrap_or_else(|| entity.entity_id.clone()),
            entity_exists,
            facts_new,
            facts_existing,
            tasks_new,
            tasks_existing,
            edges_total: entity.edges.len() as i64,
            events_new,
            events_duplicate,
            summary_action,
        });
    }
    // One warning per cause, naming THAT cause and its fix (`HoldReason`'s
    // Display) — only vocabulary causes prescribe `--fallback`.
    for (reason, ids) in &held {
        warnings.push(format!(
            "import will abort: the strict ontology gate would hold (§2.4.5) {} new entit{} \
             ({}): {reason}",
            ids.len(),
            if ids.len() == 1 { "y" } else { "ies" },
            ids.join(", ")
        ));
    }
    Ok(ImportPreview {
        profile: bundle.profile.clone(),
        entities,
        warnings,
    })
}

pub fn apply_import(
    conn: &mut Connection,
    bundle: &ParsedBundle,
    mode: ImportMode,
) -> Result<ImportResult> {
    let (now_secs, now_ms) = now_timestamps();
    // Swap (plan-p7-m4): the newtype guarantees IMMEDIATE at the type level
    // so the shared insert helper's `&ImmediateTx` parameter type-checks once
    // Task 3 calls it from `ensure_entity`. Existing callers (`tx.commit()`,
    // `&tx` → &Connection helpers) work unchanged because Transaction derefs
    // transitively to Connection. The policy load (filesystem) happens
    // BEFORE the transaction opens — r21 hold-time rule (R15).
    let policy = crate::config::ingest_policy_for_db(conn.path());
    let tx = ImmediateTx::begin(conn)?;
    let mut result = ImportResult::default();

    // #398: one column probe per import, not per event. Pre-v19 targets
    // (no forgotten_at) import without the column — unchanged behavior.
    let events_have_forgotten_at = crate::db::ddl_compat::existing_columns(&tx, "llm_wiki_events")?
        .iter()
        .any(|c| c == "forgotten_at");

    // v0.1 → v0.2 fallback (upstream §4.8): if a profile-1 fact has no `sources` key
    // but its body carries a `# Citations` list, synthesize `okf_sources` from
    // the URLs (capturing every URL, not just the first).
    let synthetic_sources = synthesize_sources_from_body(bundle);

    // Ids hard-deleted by Replace-mode `clear_entity_content`, accumulated
    // across the loop and purged from `llm_wiki_edges` once at the end.
    let mut hard_deleted_ids: Vec<String> = Vec::new();

    for entity in &bundle.entities {
        let mut id_map: HashMap<String, String> = HashMap::new();
        let target_entity_id = match mode {
            ImportMode::Clone => generate_id("ent_"),
            // Task 7 (r15-m4 / r2-m9): a bundle carries SOURCE-HOST ids; a
            // peer import of an id this host merged away must land on the
            // local survivor, else new facts attach to a loser recall can
            // never see (and cross-host re-imports recreate the duplicates
            // the merge removed). Resolution PROPAGATES errors — a DB fault
            // aborts the import, never mints a duplicate.
            _ => crate::db::entities::resolve_entity_id(&tx, &entity.entity_id)?,
        };
        if mode == ImportMode::Clone {
            for fact in &entity.facts {
                id_map.insert(fact.id.clone(), generate_id("fact_"));
            }
            for task in &entity.tasks {
                id_map.insert(task.id.clone(), generate_id("task_"));
            }
        }
        let mapped = |id: &str, map: &HashMap<String, String>| -> String {
            map.get(id).cloned().unwrap_or_else(|| id.to_string())
        };

        ensure_entity(&tx, &policy, entity, &target_entity_id, mode, now_secs)?;

        if mode == ImportMode::Replace {
            hard_deleted_ids.extend(clear_entity_content(&tx, &target_entity_id, now_ms)?);
        }

        for fact in &entity.facts {
            let fact_id = mapped(&fact.id, &id_map);
            if mode != ImportMode::Clone && fact_exists(&tx, &fact_id)? {
                result.facts_skipped += 1;
                continue;
            }
            let tags_json = serde_json::to_string(&fact.tags)?;
            // v0.1 → v0.2 fallback for `timestamp` ⇄ `generated.at`: Task 1 already
            // routes timestamp → updated_at and generated → generated_by. No code
            // change needed here — the synthesized sources sidecar covers
            // the body → okf_sources fallback.
            let effective_sources: Option<&str> = fact
                .okf_sources
                .as_deref()
                .or_else(|| synthetic_sources.get(&fact.id).map(String::as_str));
            // Pre-#186 librarian facts carry a legacy JSON `source_ref` (the
            // blob the engine setup rewrite mangled). On apply they are
            // normalized to the token shape, and a ref that STRICTLY extracts
            // a proposal_id (`proposal_id_from_evidence_json`) is salvaged
            // into the paired librarian_evidence row. Anything else —
            // engine-mangled non-JSON refs, JSON with a null/absent
            // proposal_id — is not silently destroyed: the dropped ref is
            // preserved verbatim in the result warnings (review round 5,
            // findings 3+10), matching the V18 repair's export-before-drop
            // posture. The row itself follows spec §2.3's
            // token-without-evidence stance: still-grounded, never
            // auto-purged.
            let mut legacy_evidence_json: Option<&str> = None;
            let effective_source_ref = if fact.source_type == "librarian_inferred"
                && !fact
                    .source_ref
                    .as_deref()
                    .map(is_librarian_token_shaped)
                    .unwrap_or(false)
            {
                if let Some(legacy) = fact.source_ref.as_deref() {
                    if crate::db::commit::proposal_id_from_evidence_json(legacy).is_some() {
                        legacy_evidence_json = Some(legacy);
                    } else {
                        result.warnings.push(format!(
                            "librarian fact {fact_id}: legacy source_ref not salvageable \
                             (no usable proposal_id); normalized to a fresh token, dropped \
                             ref preserved verbatim: {legacy}"
                        ));
                    }
                }
                Some(crate::db::commit::librarian_source_ref_token(&fact_id))
            } else {
                fact.source_ref.clone()
            };
            tx.execute(
                "INSERT INTO llm_wiki_entries (
                    id, entity_id, title, body, tags, confidence, source_type,
                    source_hash, source_ref, okf_type,
                    lifecycle_status, stale_after, generated_by,
                    okf_sources, okf_verified, okf_usage_window,
                    last_verified_at, last_verified_by,
                    created_at, updated_at, last_accessed_at,
                    access_count, deleted_at
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)",
                params![
                    fact_id,
                    target_entity_id,
                    fact.title,
                    fact.body,
                    tags_json,
                    fact.confidence,
                    fact.source_type,
                    fact.source_hash,
                    effective_source_ref,
                    fact.okf_type,
                    fact.lifecycle_status,
                    fact.stale_after,
                    fact.generated_by,
                    effective_sources,
                    fact.okf_verified,
                    fact.okf_usage_window,
                    fact.last_verified_at,
                    fact.last_verified_by,
                    fact.created_at,
                    fact.updated_at,
                    fact.last_accessed_at,
                    fact.access_count,
                    fact.deleted_at,
                ],
            )?;
            push_entries_outbox(
                &tx,
                &target_entity_id,
                &fact_id,
                OutboxOperation::Insert,
                wiki_fact_outbox_payload(
                    &fact_id,
                    &target_entity_id,
                    &fact.title,
                    &fact.body,
                    &fact.tags,
                    &fact.confidence,
                    &fact.source_type,
                    fact.source_hash.as_deref(),
                    effective_source_ref.as_deref().unwrap_or(""),
                    fact.okf_type.as_deref(),
                    effective_sources,
                    fact.okf_verified.as_deref(),
                    fact.okf_usage_window.as_deref(),
                    fact.created_at,
                    fact.updated_at,
                    fact.deleted_at,
                    Some(fact.lifecycle_status.as_str()),
                    fact.stale_after,
                    fact.generated_by.as_deref(),
                    fact.last_verified_at,
                    fact.last_verified_by.as_deref(),
                    // §8.4: bundles carry no area — every bundle-applied fact
                    // lands whole-repo (payload NULL, like the column).
                    None,
                ),
                now_ms,
            )?;
            // Bundle-applied librarian facts keep their token and their evidence
            // together; a token row without evidence is treated as still-grounded by
            // the §2.3 rule, but importing one deliberately would be a silent
            // provenance loss. Spec §2.3.
            if let Some(evidence_json) = fact.evidence_json.as_deref().or(legacy_evidence_json) {
                match crate::db::commit::proposal_id_from_evidence_json(evidence_json) {
                    Some(proposal_id) => {
                        // Compute the real Phase-1 flag: a bundle-applied row whose
                        // blob has no live chunk anchor must be unanchored=1, or it
                        // re-arms the heal-purge bait. Spec §2.4.
                        // Adjudication hold (PR #336 T4 delta m5 — ACCEPTED
                        // SEMANTICS, same as the evidence_repair writer): the
                        // INSERT OR REPLACE below rebuilds the row wholesale,
                        // so any prior `adjudication_hold` resets to 0 by
                        // construction. A previously held row rebuilt by a
                        // bundle import lands hold 0 and IS purge-eligible
                        // until the next re-anchor pass re-derives the class;
                        // the bundle payload's class is unknowable here.
                        let unanchored =
                            !crate::db::commit::evidence_has_live_chunk(&tx, evidence_json)?;
                        crate::db::commit::insert_librarian_evidence(
                            &tx,
                            &fact_id,
                            &proposal_id,
                            evidence_json,
                            unanchored,
                            now_ms,
                        )?;
                    }
                    None => {
                        // Strict routing (review round 5, finding 10): a blob
                        // with no usable proposal_id must not become a
                        // `proposal_id = ''` evidence row — no retraction
                        // could ever match it and no proposal could ever be
                        // re-attributed through it. Warn and skip, preserving
                        // the blob verbatim in the warnings.
                        result.warnings.push(format!(
                            "librarian fact {fact_id}: evidence blob has no usable \
                             proposal_id; evidence not attached, blob preserved \
                             verbatim: {evidence_json}"
                        ));
                    }
                }
            }
            result.facts_added += 1;
        }

        for task in &entity.tasks {
            let task_id = mapped(&task.id, &id_map);
            if mode != ImportMode::Clone && task_exists(&tx, &task_id)? {
                result.tasks_skipped += 1;
                continue;
            }
            tx.execute(
                "INSERT INTO llm_wiki_tasks (
                    id, entity_id, description, status, priority,
                    created_at, updated_at, resolved_at, deleted_at, okf_type,
                    lifecycle_status, stale_after, generated_by,
                    okf_sources, okf_verified, okf_usage_window,
                    last_verified_at, last_verified_by
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
                params![
                    task_id,
                    target_entity_id,
                    task.description,
                    task.status,
                    task.priority,
                    task.created_at,
                    task.updated_at,
                    task.resolved_at,
                    task.deleted_at,
                    task.okf_type,
                    task.lifecycle_status,
                    task.stale_after,
                    task.generated_by,
                    task.okf_sources,
                    task.okf_verified,
                    task.okf_usage_window,
                    task.last_verified_at,
                    task.last_verified_by,
                ],
            )?;
            push_tasks_outbox(
                &tx,
                &target_entity_id,
                &task_id,
                OutboxOperation::Insert,
                wiki_task_outbox_payload(
                    &task_id,
                    &target_entity_id,
                    &task.description,
                    &task.status,
                    task.priority,
                    task.created_at,
                    task.updated_at,
                    task.resolved_at,
                    task.deleted_at,
                    task.okf_type.as_deref(),
                    task.okf_sources.as_deref(),
                    task.okf_verified.as_deref(),
                    task.okf_usage_window.as_deref(),
                    Some(task.lifecycle_status.as_str()),
                    task.stale_after,
                    task.generated_by.as_deref(),
                    task.last_verified_at,
                    task.last_verified_by.as_deref(),
                ),
                now_ms,
            )?;
            result.tasks_added += 1;
        }

        for (source, target, edge_type) in &entity.edges {
            let source_id = mapped(source, &id_map);
            let target_id = mapped(target, &id_map);

            // The import path is the SECOND production writer of
            // `llm_wiki_edges` (the first is `commit_edge_add`), so it needs
            // the same liveness contract: an endpoint the purger calls dead
            // must not be written. `purge_dead_edges` below only collects an
            // edge once BOTH endpoints are dead — a **half-live** edge is
            // deliberately retained (edge_purge module docs) — so a dead
            // endpoint paired with a live one dangles for as long as its live
            // partner survives, and no later cascade ever collects it.
            //
            // The reachable case needs no malformed bundle: `fact_exists` /
            // `task_exists` test only `SELECT 1 ... WHERE id=?1`, with no
            // `deleted_at` gate, so in merge/replace mode a bundle row whose
            // id is already present but **soft-deleted** in the destination is
            // counted as existing and skipped — it stays a tombstone. An edge
            // naming it would then point at a dead row while its partner
            // stayed live.
            //
            // Both endpoints are always ids from this same entity's bundle
            // directory (`bundle_read` builds `concept_ids` per entity dir and
            // drops unresolvable links at parse), and this entity's facts and
            // tasks are written by the two loops above, so a live endpoint is
            // already visible here — the check cannot drop a legitimate
            // forward reference.
            //
            // Skip-and-warn rather than fail: the same drop-don't-fail
            // contract `resolve_edge_ref` uses on the commit path, and the
            // warning carries both ids so the loss is attributable.
            if !endpoint_is_live(&tx, &source_id)? || !endpoint_is_live(&tx, &target_id)? {
                result.warnings.push(format!(
                    "dropped edge {source_id} -{edge_type}-> {target_id}: an endpoint names no \
                     live row in llm_wiki_entries, curated_entities, or llm_wiki_tasks"
                ));
                continue;
            }

            let inserted = tx.execute(
                "INSERT OR IGNORE INTO llm_wiki_edges
                    (id, entity_id, source_id, target_id, edge_type, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    generate_id("edge_"),
                    target_entity_id,
                    source_id,
                    target_id,
                    edge_type,
                    now_ms,
                ],
            )?;
            result.edges_added += inserted as i64;
        }

        // T4 r1 m2 + r4 m3: one cluster resolution per entity, lazily computed
        // on the first event with a related entry that needs it (every fresh
        // insert runs the linkage gate).
        let mut entity_cluster_cache: Option<Vec<String>> = None;
        for ev in &entity.events {
            let (event_id, is_dup) = match mode {
                ImportMode::Clone => (generate_id("evt_"), false),
                _ => {
                    let dup = event_exists(
                        &tx,
                        &target_entity_id,
                        ev.event_id.as_deref(),
                        &ev.event_type,
                        &ev.summary,
                        &ev.date,
                    )?;
                    (
                        ev.event_id.clone().unwrap_or_else(|| generate_id("evt_")),
                        dup,
                    )
                }
            };
            if is_dup {
                if events_have_forgotten_at {
                    if let (Some(ms), Some(id)) = (ev.forgotten_at, ev.event_id.as_deref()) {
                        // #398: a dup whose source row is forgotten must tombstone the
                        // target's live row — otherwise an already-synced brain keeps
                        // the forgotten event alive forever (the exact resurrection
                        // #398 fixes). Pre-v19 targets skip this (no column to set).
                        // Tombstone wins on conflict (spec r1-M3): only a live target
                        // row is tombstoned; a target row already forgotten is never
                        // un-forgotten by an incoming live event. The reconciled dup
                        // counts ONLY as events_skipped — the tombstone is
                        // intentionally silent in ImportResult (r3 minor 6).
                        // Scoped to the import's entity CLUSTER (T1.5 r1 MAJOR +
                        // CodeRabbit CR-1): a colliding id may belong to another
                        // entity's row — bundle content must never mutate it —
                        // but a row owned by a merged-away loser (entity_id =
                        // loser, redirect → target) IS part of this import
                        // (export loads the whole cluster via entity_id IN),
                        // so scoping to the survivor alone would leave the
                        // forgotten event live and resurrect its entry.
                        let cluster = entity_cluster(
                            &mut entity_cluster_cache,
                            &tx,
                            &target_entity_id,
                        )?;
                        let placeholders =
                            crate::db::entities::in_placeholders(&cluster);
                        // T4 r4 M1: the tombstone must also refuse a row whose
                        // LINKAGE names an entry outside the cluster (legacy
                        // rows imported by pre-gate builds can still carry
                        // foreign linkage) — tombstoning such a row would hide
                        // a foreign entry. related_entry_id is an ENTRY id, so
                        // the ownership check resolves it through
                        // llm_wiki_entries (T4 r4 verification: comparing it
                        // to entity ids directly never matches). NULL or
                        // cluster-owned entries pass.
                        let sql = format!(
                            "UPDATE llm_wiki_events SET forgotten_at = ?1 \
                             WHERE id = ?2 AND forgotten_at IS NULL \
                             AND entity_id IN ({placeholders}) \
                             AND (related_entry_id IS NULL OR EXISTS ( \
                                  SELECT 1 FROM llm_wiki_entries le \
                                  WHERE le.id = llm_wiki_events.related_entry_id \
                                  AND le.entity_id IN ({placeholders})))"
                        );
                        let mut params: Vec<&dyn rusqlite::ToSql> =
                            vec![&ms, &id];
                        params.extend(cluster.iter().map(|c| c as &dyn rusqlite::ToSql));
                        params.extend(cluster.iter().map(|c| c as &dyn rusqlite::ToSql));
                        tx.execute(&sql, params.as_slice())?;
                    }
                }
                result.events_skipped += 1;
                continue;
            }
            let created_at = ms_from_utc_date(&ev.date).unwrap_or(now_ms);
            let mut related = ev.related_entry_id.as_deref().map(|id| mapped(id, &id_map));
            // T4 r1-r4 (M1/M2 across rounds): bundle content must not flip the
            // VISIBILITY of an entry it does not own. The survival guard is
            // entity-agnostic — a FORGOTTEN event with a related id hides that
            // entry (unless something else backs it live), and a LIVE event
            // with a related id can resurrect (un-hide) a locally-forgotten
            // one. So the LINKAGE is kept only when the related entry is owned
            // by the import's CLUSTER (survivor ∪ redirect losers — the same
            // set export loads); otherwise it is dropped
            // (related_entry_id = NULL) on EVERY fresh insert, live or
            // forgotten, on EVERY shape (pre-v19 shapes too — the tables this
            // gate queries are template tables, and a shape that gains the
            // forgotten_at column later would inherit legacy linkage).
            // Task-related ids are kept (the guard joins llm_wiki_entries; task
            // ids are a distinct namespace). The dropped linkage is REPORTED
            // here and the loss is permanent. The tombstone itself is always
            // applied verbatim; with no linkage a forgotten event can neither
            // hide nor back any entry, and a re-import dup is a no-op
            // (tombstone-wins). (Downgrading to a live insert was rejected in
            // T4 r2; scoping the gate to forgotten inserts only was rejected
            // in T4 r3 — the live-then-forgotten two-import route reached the
            // same hide; conditioning on events_have_forgotten_at was rejected
            // in T4 r4 — pre-v19 imports would mint legacy linkage.)
            {
                let mut linkage_reason: Option<String> = None;
                if let Some(rid) = related.as_deref() {
                    let owner: Option<String> = tx
                        .query_row(
                            "SELECT entity_id FROM llm_wiki_entries WHERE id=?1",
                            [&rid],
                            |r| r.get(0),
                        )
                        .optional()?;
                    match owner {
                        Some(o) => {
                            // T4 r4 m4: check the cache without cloning it —
                            // this runs for every fresh insert with a related
                            // entry.
                            let in_cluster = match entity_cluster_cache.as_ref() {
                                Some(c) => c.iter().any(|c| c == &o),
                                None => {
                                    let c = crate::db::entities::cluster_ids(
                                        &tx,
                                        &target_entity_id,
                                    )?;
                                    let hit = c.iter().any(|c| c == &o);
                                    entity_cluster_cache = Some(c);
                                    hit
                                }
                            };
                            if !in_cluster {
                                linkage_reason = Some(format!(
                                    "related entry {rid} is owned outside the import's entity cluster — linkage dropped and reported here (the loss is permanent); bundle content cannot change the visibility of entries outside the import's entity cluster"
                                ));
                            }
                        }
                        None => {
                            // Not a local entry. A TASK-related id cannot
                            // hide anything (the guard joins llm_wiki_entries;
                            // task ids are a distinct namespace) — keep it. An
                            // id that exists nowhere yet could name an entry
                            // imported LATER, so its linkage is dropped.
                            let is_task = tx
                                .query_row(
                                    "SELECT 1 FROM llm_wiki_tasks WHERE id=?1",
                                    [&rid],
                                    |r| r.get::<_, i64>(0),
                                )
                                .optional()?
                                .is_some();
                            if !is_task {
                                linkage_reason = Some(format!(
                                    "related entry {rid} not found locally and could name an entry imported later — linkage dropped and reported here (the loss is permanent); bundle content cannot change the visibility of entries it does not own"
                                ));
                            }
                        }
                    }
                }
                if let Some(reason) = linkage_reason {
                    let mut warning = format!("event {event_id}: {reason}");
                    if ev.forgotten_at.is_some() {
                        // T4 r4 m5: the tombstone-kept reassurance belongs on
                        // forgotten events specifically.
                        warning.push_str("; the tombstone is kept verbatim, and with no linkage it can neither hide nor back any entry");
                    }
                    result.warnings.push(warning);
                    related = None;
                }
            }
            if events_have_forgotten_at {
                tx.execute(
                    "INSERT OR IGNORE INTO llm_wiki_events
                        (id, entity_id, event_type, summary, related_entry_id, created_at, forgotten_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        event_id,
                        target_entity_id,
                        ev.event_type,
                        ev.summary,
                        related,
                        created_at,
                        ev.forgotten_at
                    ],
                )?;
            } else {
                tx.execute(
                    "INSERT OR IGNORE INTO llm_wiki_events
                        (id, entity_id, event_type, summary, related_entry_id, created_at)
                     VALUES (?1,?2,?3,?4,?5,?6)",
                    params![
                        event_id,
                        target_entity_id,
                        ev.event_type,
                        ev.summary,
                        related,
                        created_at
                    ],
                )?;
            }
            result.events_added += 1;
        }

        tx.execute(
            "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, related_entry_id, created_at)
             VALUES (?1,?2,'imported',?3,NULL,?4)",
            params![
                generate_id("evt_"),
                target_entity_id,
                format!(
                    "OKF import: bundle contains {} fact(s), {} task(s)",
                    entity.facts.len(),
                    entity.tasks.len()
                ),
                now_ms,
            ],
        )?;
        result.entities_touched += 1;
    }

    // Edges stamped with any of the replaced entities' ids may point across
    // entities; purge only edges whose BOTH endpoints have no live home
    // (in llm_wiki_entries / curated_entities / llm_wiki_tasks), otherwise
    // the bundle import strands partner edges (remediation brief R1). Done
    // once after the loop instead of once per entity in
    // `clear_entity_content` — the per-entity call was an unscoped DELETE
    // over `llm_wiki_edges` per row, so for a bundle of N entities we were
    // running N scans where one suffices (CodeRabbit thread on line 641).
    // Replace mode HARD-deletes the entity's entries and tasks. Those ids are
    // gone from every endpoint table, so an edge anchored on one references
    // nothing and no later cascade could ever find it — `purge_dead_edges`
    // below needs BOTH endpoints dead and would leave it behind forever.
    // Batched once after the loop, for the same reason as `purge_dead_edges`.
    if !hard_deleted_ids.is_empty() {
        purge_edges_for_hard_deleted(&tx, &hard_deleted_ids)?;
    }

    purge_dead_edges(&tx)?;

    tx.commit()?;

    // §8.4: ONE loud summary line per import stating the bundle area posture.
    // Emitted AFTER `tx.commit()?` (review round 1 M1): a failed commit rolls
    // the import back and the operator must never read "N facts imported" for
    // rows that do not exist. Routed through the db/ warn channel pattern
    // (eprintln! non-feature + tracing under `mcp-server`, review round 1 m1)
    // AND surfaced in `ImportResult.warnings` — GUI operators never see
    // stdout, and JSON-RPC adjacency makes bare stdout writes hazardous.
    let line = bundle_area_summary_line(result.facts_added);
    emit_bundle_area_summary(&line);
    result.warnings.push(line);
    Ok(result)
}

/// §8.4: the per-import summary line. Printed EXACTLY ONCE per `apply_import`
/// (single call site, after the entity loop) so the operator sees the bundle
/// area posture on every import: bundles carry no area; everything lands
/// whole-repo regardless of the source host's scoping.
pub(crate) fn bundle_area_summary_line(facts_added: i64) -> String {
    format!("{facts_added} facts imported; bundles carry no area; all land whole-repo")
}

/// The db/ channel pattern (see `commit.rs::warn_source_ref_parse_error`):
/// a `tracing` event under `mcp-server` (where a subscriber exists) and
/// `eprintln!` in the Tauri build, which has none.
#[cfg(feature = "mcp-server")]
fn emit_bundle_area_summary(line: &str) {
    tracing::info!(target: "ct::bundle_apply", "{line}");
}

#[cfg(not(feature = "mcp-server"))]
fn emit_bundle_area_summary(line: &str) {
    eprintln!("{line}");
}

fn ensure_entity(
    tx: &ImmediateTx<'_>,
    policy: &crate::config::IngestPolicy,
    entity: &ParsedEntity,
    target_entity_id: &str,
    mode: ImportMode,
    now_secs: i64,
) -> Result<()> {
    let name = entity
        .display_name
        .clone()
        .unwrap_or_else(|| entity.entity_id.clone());
    let bundle_summary = entity.summary.clone().unwrap_or_default();
    // r2-m9: the existence probe PROPAGATES DB faults (the pre-Task-7
    // `.ok()` swallowed them as "not found", so a transient fault during
    // import minted a duplicate). `optional()` keeps only the genuinely
    // no-row case as `None`. The caller already resolved the id through
    // the redirect chain, so this probe is on a terminal survivor.
    let existing: Option<String> = tx
        .query_row(
            "SELECT summary FROM curated_entities WHERE id=?1",
            [target_entity_id],
            |r| r.get(0),
        )
        .optional()?;

    // Bundle import path (spec §2.5 / R2.4.2). Wave-1 bundles carry NO
    // graph type label — the gate's caller supplies `None` for the
    // proposed label. The four §2.5 ladder branches are summarized below.
    //
    // * Gate → unlabeled entity → insert as `fallback_node_type`
    //   (NEVER the literal `'concept'`). Write an `unlabeled_landing` row
    //   in the origin ledger; the entity goes to the review queue.
    // * Gate → no declared fallback (Held) → atomic ABORT. Per spec §2.5
    //   bundle bullet: "If no fallback is declared, the import ABORTS
    //   ATOMICALLY with a report, facts intact." We bail here and the
    // outer IMMEDIATE transaction rolls back the bundle.
    // * Skip → SKIP-path `'concept'` literal + `gate_skipped` row.
    //   source_directory is NULL here (the §2.5 bundle bullet never sets
    //   a source directory because bundle import has no document path).
    // * Skip on a label-less entity (per spec R2.4.6 r21 "SKIP on
    //   bundle import (no label)"): the gate returns Skipped with
    //   `original_label: None`; we land `'concept'` and write a
    //   `gate_skipped` row with original_type = NULL.
    if existing.is_none() {
        let (decision, _gate) =
            crate::db::entity_gate::resolve_production_gate(tx, policy, target_entity_id, &[]);
        let outcome = crate::db::entity_gate::shared_insert_entity(
            tx,
            Some(target_entity_id),
            &name,
            None,
            &bundle_summary,
            now_secs,
            decision.clone(),
            false,
        )?;

        use crate::db::entity_gate::AdmitOutcome;
        use crate::db::entity_gate::GateDecision;
        use crate::db::schema::OriginReason;

        match (&outcome, &decision) {
            (AdmitOutcome::DegradedToFallback { landed_as, .. }, _) => {
                // Degraded/unlabeled landing: helper inserted as the
                // declared fallback. Write an `unlabeled_landing` ledger
                // row (original_type NULL — r21 contract).
                crate::db::entity_gate::write_origin_ledger_row(
                    tx,
                    target_entity_id,
                    None,
                    OriginReason::UnlabeledLanding,
                    None,
                )?;
                let _ = landed_as;
            }
            (AdmitOutcome::Skipped { .. }, GateDecision::Skip) => {
                // SKIP path: today's `'concept'` literal stands (spec
                // §2.5 bundle bullet, r2-M2a). Helper did NOT insert —
                // land the literal ourselves, then write the
                // `gate_skipped` ledger row (original_type NULL when no
                // label was supplied, r21 table).
                tx.execute(
                    "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
                     VALUES (?1, ?2, 'concept', ?3, ?4, ?4)",
                    params![target_entity_id, name, bundle_summary, now_secs],
                )?;
                crate::db::entity_gate::write_origin_ledger_row(
                    tx,
                    target_entity_id,
                    None,
                    OriginReason::GateSkipped,
                    None,
                )?;
            }
            (AdmitOutcome::Held { reason, .. }, _) => {
                // §2.5 bullet: "If no fallback is declared, the import
                // ABORTS ATOMICALLY with a report, facts intact." The
                // outer IMMEDIATE transaction rolls back via bail!
                // (caller propagates → apply_import returns Err →
                // tx.rollback()). Facts intact because the LLM-mint
                // path is in the same batch.
                bail!(
                    "bundle import aborted: strict ontology gate held (§2.4.5) an unlabeled \
                     entity mint: {reason}. Nothing was imported; re-run the import once the \
                     cause is fixed"
                );
            }
            _ => {
                // Admitted-as-declared or alias-admitted without a label —
                // nothing the bundle entry needed, but the helper
                // inserted anyway. We don't write a ledger row.
            }
        }
        return Ok(());
    }

    // Existing row: only the summary update path remains.
    let local_summary = existing.unwrap_or_default();
    let write_summary = match mode {
        ImportMode::Replace => entity.summary.is_some(),
        ImportMode::Merge => entity.summary.is_some() && local_summary.trim().is_empty(),
        ImportMode::Clone => false,
    };
    if write_summary {
        tx.execute(
            "UPDATE curated_entities SET summary=?2, updated_at=?3 WHERE id=?1",
            params![target_entity_id, bundle_summary, now_secs],
        )?;
    }
    Ok(())
}

/// Wipe an entity's entries, tasks and events for a Replace-mode import.
///
/// Returns the ids that were **hard**-deleted (entries + tasks) so the caller
/// can purge their edges once, after the entity loop — see the call site in
/// `apply_import`.
/// Replace-mode wipe of `entity_id`'s content, CLUSTER-CLOSED: the target
/// is a (possibly merge-resolved) survivor, and facts/tasks/events still
/// keyed to a redirect loser are part of what recall shows for it (spec
/// R2.7.5 transitive fact closure). Wiping only `entity_id = survivor`
/// would leave the loser-keyed rows alive beside the imported copies and
/// push no Delete outbox rows for them (the #132 class).
fn clear_entity_content(tx: &Connection, entity_id: &str, now_ms: i64) -> Result<Vec<String>> {
    let mut hard_deleted = Vec::new();
    for member in crate::db::entities::cluster_ids(tx, entity_id)? {
        hard_deleted.extend(clear_one_entity_content(tx, &member, now_ms)?);
    }
    Ok(hard_deleted)
}

/// Wipe the rows keyed to exactly `entity_id` — see [`clear_entity_content`].
/// Outbox rows carry the row's own owner id so replicas address the row
/// they actually hold.
fn clear_one_entity_content(tx: &Connection, entity_id: &str, now_ms: i64) -> Result<Vec<String>> {
    let fact_ids: Vec<String> = tx
        .prepare("SELECT id FROM llm_wiki_entries WHERE entity_id=?1")?
        .query_map([entity_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in &fact_ids {
        push_entries_outbox(
            tx,
            entity_id,
            id,
            OutboxOperation::Delete,
            serde_json::json!({ "id": id }),
            now_ms,
        )?;
    }
    let task_ids: Vec<String> = tx
        .prepare("SELECT id FROM llm_wiki_tasks WHERE entity_id=?1")?
        .query_map([entity_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in &task_ids {
        push_tasks_outbox(
            tx,
            entity_id,
            id,
            OutboxOperation::Delete,
            serde_json::json!({ "id": id }),
            now_ms,
        )?;
    }
    // FK CASCADE is not relied upon (spec §2.1): brain.db has connections whose
    // `PRAGMA foreign_keys` state we do not control, so the evidence row is
    // deleted explicitly alongside its entry.
    tx.execute(
        "DELETE FROM librarian_evidence WHERE entry_id IN
             (SELECT id FROM llm_wiki_entries WHERE entity_id = ?1)",
        [entity_id],
    )?;
    tx.execute(
        "DELETE FROM llm_wiki_entries WHERE entity_id=?1",
        [entity_id],
    )?;
    tx.execute("DELETE FROM llm_wiki_tasks WHERE entity_id=?1", [entity_id])?;
    // NOTE: neither `purge_dead_edges` nor `purge_edges_for_hard_deleted` is
    // called here. Both run once after the entire entity loop (in
    // `apply_import`) so the scan happens once per import rather than once
    // per entity — see CodeRabbit review thread on `bundle_apply.rs` line 641.
    // That is why the hard-deleted ids are returned rather than acted on.
    tx.execute(
        "DELETE FROM llm_wiki_events WHERE entity_id=?1",
        [entity_id],
    )?;
    let mut hard_deleted = fact_ids;
    hard_deleted.extend(task_ids);
    Ok(hard_deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::okf::bundle_read::{ParsedBundle, ParsedEntity, ParsedEvent};
    use crate::okf::types::WikiFact;

    /// engine-9x PR-B (spec §3 step 6): the raw entity wipe
    /// (`clear_one_entity_content`, `DELETE FROM llm_wiki_entries WHERE
    /// entity_id=?1`) must not bypass the vector-removal ledger triggers —
    /// over a migrated 8.0.2 fixture copy, unmarked live hard deletes enqueue
    /// nothing; with `external_index_in_use = '1'` a pre-existing (soft
    /// delete) row survives and the live fact enqueues exactly one row.
    #[test]
    fn t9_clear_entity_content_does_not_bypass_the_ledger() {
        let rows = |c: &Connection, id: &str| -> i64 {
            c.query_row(
                "SELECT COUNT(*) FROM llm_wiki_vector_removals WHERE fact_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        for marker in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let dst = tmp.path().join("brain.db");
            std::fs::copy(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/db/testdata/ct-brain-v32-core-8.0.2.db"),
                &dst,
            )
            .unwrap();
            let conn = Connection::open(&dst).unwrap();
            crate::db::connection::migrate_open_db(&conn, Some(tmp.path())).unwrap();
            if marker {
                conn.execute(
                    "INSERT OR REPLACE INTO llm_wiki_meta (key, value) VALUES ('external_index_in_use', '1')",
                    [],
                )
                .unwrap();
            }
            for id in ["t9_soft", "t9_live"] {
                conn.execute(
                    "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at)
                     VALUES (?1, 'ent_t9', 't', 'b', 1, 1)",
                    [id],
                )
                .unwrap();
            }
            conn.execute("UPDATE llm_wiki_entries SET deleted_at = 5 WHERE id = 't9_soft'", [])
                .unwrap();
            assert_eq!(rows(&conn, "t9_soft"), 1);
            clear_one_entity_content(&conn, "ent_t9", 1).unwrap();
            let left: i64 = conn
                .query_row("SELECT COUNT(*) FROM llm_wiki_entries WHERE entity_id = 'ent_t9'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(left, 0, "entity wiped");
            let expected = i64::from(marker);
            assert_eq!(rows(&conn, "t9_soft"), expected, "marker={marker}: soft row");
            assert_eq!(rows(&conn, "t9_live"), expected, "marker={marker}: live row");
        }
    }

    fn sample_fact(id: &str, entity_id: &str) -> WikiFact {
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

    fn sample_bundle() -> ParsedBundle {
        ParsedBundle {
            profile: Some(LLM_WIKI_PROFILE_V2.into()),
            entities: vec![ParsedEntity {
                entity_id: "ent_a".into(),
                display_name: Some("Project X".into()),
                summary: Some("Bundle summary.".into()),
                facts: vec![sample_fact("fact_1", "ent_a")],
                tasks: vec![],
                edges: vec![],
                events: vec![ParsedEvent {
                    event_id: Some("evt_1".into()),
                    event_type: "action".into(),
                    summary: "Did a thing".into(),
                    related_entry_id: Some("fact_1".into()),
                    date: "2026-07-05".into(),
                    forgotten_at: None,
                }],
            }],
            ..ParsedBundle::default()
        }
    }

    /// §8.4: the summary line's exact wording is a pin — the rename task (Task
    /// 9) and log scrapers lean on it.
    #[test]
    fn bundle_area_summary_line_wording_is_pinned() {
        assert_eq!(
            bundle_area_summary_line(3),
            "3 facts imported; bundles carry no area; all land whole-repo"
        );
        assert_eq!(
            bundle_area_summary_line(0),
            "0 facts imported; bundles carry no area; all land whole-repo"
        );
    }

    #[test]
    fn merge_creates_entity_and_inserts_new_rows() {
        let mut conn = open_in_memory().unwrap();
        let result = apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        assert_eq!(result.facts_added, 1);
        assert_eq!(result.events_added, 1);
        let summary: String = conn
            .query_row(
                "SELECT summary FROM curated_entities WHERE id='ent_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(summary, "Bundle summary.");
        let outbox: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_outbox WHERE table_name='entries'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(outbox, 1);
    }

    /// A bundle whose edge names two live endpoints must still write. Guards
    /// the liveness check against over-restricting the import path: both
    /// endpoints are this entity's own facts, written by the loop above.
    #[test]
    fn merge_writes_edge_between_live_endpoints() {
        let mut conn = open_in_memory().unwrap();
        let mut bundle = sample_bundle();
        bundle.entities[0]
            .facts
            .push(sample_fact("fact_2", "ent_a"));
        bundle.entities[0]
            .edges
            .push(("fact_1".into(), "fact_2".into(), "relates_to".into()));

        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        assert_eq!(result.edges_added, 1, "a live-to-live edge must be written");
        // Review round 1 m1: the §8.4 summary line now rides `warnings`
        // (exactly-once asserted in areas_outbox.rs); no OTHER warning here.
        assert_eq!(
            result.warnings.len(),
            1,
            "only the summary line expected: {:?}",
            result.warnings
        );
        assert_eq!(
            result.warnings[0],
            bundle_area_summary_line(result.facts_added)
        );
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges
                  WHERE source_id='fact_1' AND target_id='fact_2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    /// The import path is the second production writer of `llm_wiki_edges`
    /// and needs `commit_edge_add`'s liveness contract.
    ///
    /// No malformed bundle is required. `fact_exists` has no `deleted_at`
    /// gate, so a merge whose fact id is already present but **soft-deleted**
    /// counts it as existing and skips it — the row stays a tombstone. The
    /// edge naming it would then be half-live (live `fact_1`, dead `fact_2`),
    /// and `purge_dead_edges` only collects an edge once BOTH endpoints are
    /// dead, so pre-fix it dangled for the life of its live partner.
    #[test]
    fn merge_refuses_edge_to_tombstoned_endpoint() {
        let mut conn = open_in_memory().unwrap();
        let mut bundle = sample_bundle();
        bundle.entities[0]
            .facts
            .push(sample_fact("fact_2", "ent_a"));
        bundle.entities[0]
            .edges
            .push(("fact_1".into(), "fact_2".into(), "relates_to".into()));

        // Seed the destination with a soft-deleted `fact_2`: the import will
        // count it as existing, skip it, and leave the tombstone in place.
        apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();
        conn.execute("DELETE FROM llm_wiki_edges", []).unwrap();
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 1234 WHERE id = 'fact_2'",
            [],
        )
        .unwrap();

        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        assert_eq!(
            result.edges_added, 0,
            "an edge naming a tombstoned endpoint must not be written"
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "no half-live edge may survive the import");
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("fact_2") && w.contains("dropped edge")),
            "the drop must be attributable: {:?}",
            result.warnings
        );
    }

    #[test]
    fn merge_is_idempotent_id_first() {
        let mut conn = open_in_memory().unwrap();
        apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        let second = apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        assert_eq!(second.facts_added, 0);
        assert_eq!(second.facts_skipped, 1);
        assert_eq!(second.events_added, 0);
        let facts: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(facts, 1);
    }

    #[test]
    fn merge_tuple_dedup_for_profile0_events() {
        let mut conn = open_in_memory().unwrap();
        apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        let mut profile0 = sample_bundle();
        profile0.profile = None;
        profile0.entities[0].facts.clear();
        profile0.entities[0].events[0].event_id = None;
        let result = apply_import(&mut conn, &profile0, ImportMode::Merge).unwrap();
        assert_eq!(
            result.events_added, 0,
            "tuple (type, summary, day) already present"
        );
    }

    #[test]
    fn replace_clears_then_inserts() {
        let mut conn = open_in_memory().unwrap();
        apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence, source_type, created_at, updated_at)
             VALUES ('fact_local', 'ent_a', 'Local only', 'x', '[]', 'inferred', 'librarian_inferred', 1, 1)",
            [],
        )
        .unwrap();
        apply_import(&mut conn, &sample_bundle(), ImportMode::Replace).unwrap();
        let ids: Vec<String> = conn
            .prepare("SELECT id FROM llm_wiki_entries WHERE entity_id='ent_a'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            ids,
            vec!["fact_1".to_string()],
            "local-only fact removed by replace"
        );
    }

    #[test]
    fn replace_import_carries_forgotten_at_on_fresh_insert() {
        // T1.5 r2 MINOR 3: Replace clears every CLUSTER member
        // (clear_entity_content loops cluster_ids, losers included), so for
        // the target's own rows the dup path is unreachable — a bundle-only
        // event is a fresh INSERT, and the forgotten flag must ride it.
        // Merge and Replace share the carry-the-flag contract (spec).
        let mut conn = open_in_memory().unwrap();
        let mut bundle = sample_bundle();
        bundle.entities[0].events[0].forgotten_at = Some(1732713600000);
        apply_import(&mut conn, &bundle, ImportMode::Replace).unwrap();
        let forgotten: Option<i64> = conn
            .query_row(
                "SELECT forgotten_at FROM llm_wiki_events WHERE id='evt_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            forgotten,
            Some(1732713600000),
            "fresh INSERT in Replace mode carries forgotten_at verbatim"
        );
    }

    #[test]
    fn clone_remaps_every_id() {
        let mut conn = open_in_memory().unwrap();
        apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        let result = apply_import(&mut conn, &sample_bundle(), ImportMode::Clone).unwrap();
        assert_eq!(result.facts_added, 1);
        let entities: i64 = conn
            .query_row("SELECT COUNT(*) FROM curated_entities", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entities, 2);
        let fact1: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id='fact_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fact1, 1);
        let evt1: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_events WHERE id='evt_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(evt1, 1, "clone must remap event ids too");
        let dangling: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_events e
                 WHERE e.related_entry_id IS NOT NULL
                 AND NOT EXISTS (SELECT 1 FROM llm_wiki_entries f WHERE f.id = e.related_entry_id)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dangling, 0);
    }

    #[test]
    fn replace_on_merge_survivor_wipes_loser_keyed_content() {
        // Review finding: the bundle's `ent_a` was merged into `ent_s` on
        // this host, so Replace resolves to the survivor. A fact still keyed
        // to the loser is part of the survivor's cluster and must be wiped
        // (with a Delete outbox row) like the survivor's own.
        let mut conn = open_in_memory().unwrap();
        for id in ["ent_a", "ent_s"] {
            conn.execute(
                "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
                 VALUES (?1, ?1, 'concept', '', 1, 1)",
                [id],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES ('ent_a', 'ent_s', 1)",
            [],
        )
        .unwrap();
        for (fact_id, entity_id) in [("fact_loser", "ent_a"), ("fact_surv", "ent_s")] {
            conn.execute(
                "INSERT INTO llm_wiki_entries (
                    id, entity_id, title, body, tags, confidence, source_type,
                    created_at, updated_at
                 ) VALUES (?1, ?2, 'T', 'B', '[]', 'inferred', 'librarian_inferred', 1, 1)",
                params![fact_id, entity_id],
            )
            .unwrap();
        }

        apply_import(&mut conn, &sample_bundle(), ImportMode::Replace).unwrap();

        let ids: Vec<String> = conn
            .prepare("SELECT id FROM llm_wiki_entries ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(ids, vec!["fact_1".to_string()]);
        let loser_delete: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_outbox
                 WHERE table_name='entries' AND record_id='fact_loser' AND operation='DELETE'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(loser_delete, 1);
    }

    #[test]
    fn replace_keeps_edges_stamped_with_the_entity_whose_endpoints_live_elsewhere() {
        // R1 proof: `clear_entity_content` (called by Replace mode) used to
        // DELETE every llm_wiki_edges row stamped with the dying entity_id,
        // even when the edge points into another live entity. That strands the
        // partner's edges. The `entity_id` STAMP must never decide an edge's
        // fate — only its endpoints do.
        //
        // Note the distinction this test now pins, which the earlier version
        // conflated: an edge stamped `ent_a` whose endpoints both live in
        // other entities survives a Replace on `ent_a`; an edge with an
        // endpoint IN `ent_a` does not, because Replace hard-deletes that
        // endpoint and the edge would reference a row that no longer exists in
        // any table (see `replace_purges_edges_anchored_on_its_hard_deleted_facts`).
        let mut conn = open_in_memory().unwrap();

        // Two entities, each with a fact.
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_a', 'A', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_b', 'B', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        for (fact_id, entity_id) in [
            ("fact_a_1", "ent_a"),
            ("fact_b_1", "ent_b"),
            ("fact_b_2", "ent_b"),
        ] {
            conn.execute(
                "INSERT INTO llm_wiki_entries (
                    id, entity_id, title, body, tags, confidence, source_type,
                    created_at, updated_at
                 ) VALUES (?1, ?2, 'T', 'B', '[]', 'inferred', 'librarian_inferred', 1, 1)",
                params![fact_id, entity_id],
            )
            .unwrap();
        }

        // Stamped with ent_a, but BOTH endpoints are ent_b's facts. The stamp
        // is the only thing tying it to the replaced entity, so it MUST
        // survive — this is the R1 regression guard.
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_cross', 'ent_a', 'fact_b_1', 'fact_b_2', 'related_to', 1757000000000)",
            [],
        )
        .unwrap();
        // Truly dead edge: neither endpoint exists anywhere.
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_orphan', 'ent_a', 'ghost_a', 'ghost_b', 'related_to', 1757000000000)",
            [],
        )
        .unwrap();

        // Replace ent_a — clears ent_a's facts/edges/etc.
        apply_import(&mut conn, &sample_bundle(), ImportMode::Replace).unwrap();

        let surviving_ids: Vec<String> = conn
            .prepare("SELECT id FROM llm_wiki_edges ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            surviving_ids,
            vec!["edge_cross".to_string()],
            "an edge merely STAMPED with the replaced entity survives when both \
             its endpoints live elsewhere; the orphan edge is purged because \
             both endpoints are dead everywhere"
        );
    }

    #[test]
    fn replace_purges_edges_anchored_on_its_hard_deleted_facts() {
        // Replace HARD-deletes the entity's facts. An edge from one of them
        // into a live fact in another entity references a row that is gone
        // from every endpoint table — the `purge_dead_edges` contract needs
        // BOTH endpoints dead, so nothing would ever collect it and it would
        // dangle for the life of the database.
        let mut conn = open_in_memory().unwrap();
        for (entity_id, name) in [("ent_a", "A"), ("ent_b", "B")] {
            conn.execute(
                "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
                 VALUES (?1, ?2, 'concept', '', 1, 1)",
                params![entity_id, name],
            )
            .unwrap();
        }
        for (fact_id, entity_id) in [("fact_a_1", "ent_a"), ("fact_b_1", "ent_b")] {
            conn.execute(
                "INSERT INTO llm_wiki_entries (
                    id, entity_id, title, body, tags, confidence, source_type,
                    created_at, updated_at
                 ) VALUES (?1, ?2, 'T', 'B', '[]', 'inferred', 'librarian_inferred', 1, 1)",
                params![fact_id, entity_id],
            )
            .unwrap();
        }
        // One in each direction, both anchored on the fact Replace destroys.
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_out', 'ent_a', 'fact_a_1', 'fact_b_1', 'related_to', 1757000000000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_in', 'ent_b', 'fact_b_1', 'fact_a_1', 'related_to', 1757000000000)",
            [],
        )
        .unwrap();

        apply_import(&mut conn, &sample_bundle(), ImportMode::Replace).unwrap();

        let surviving: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            surviving, 0,
            "edges anchored on a hard-deleted fact must not survive the import \
             that destroyed it, in either direction"
        );
        // The surviving partner is untouched.
        let partner: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'fact_b_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(partner, 1);
    }

    #[test]
    fn preview_counts_without_writing() {
        let mut conn = open_in_memory().unwrap();
        apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        let preview = preview_import(&conn, &sample_bundle(), ImportMode::Merge).unwrap();
        assert_eq!(preview.entities.len(), 1);
        assert_eq!(preview.entities[0].facts_new, 0);
        assert_eq!(preview.entities[0].facts_existing, 1);
        assert_eq!(preview.entities[0].events_duplicate, 1);
    }

    /// Preview and apply agree: on a strict tier_fact with no usable
    /// fallback the apply aborts on the held mint, so the preview must warn
    /// instead of reporting the bundle importable.
    #[test]
    fn preview_warns_when_apply_would_hold() {
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('tier_fact', 'strict', '{\"node_types\":[{\"type\":\"person\"}],\"edge_types\":[]}', 1)",
            [],
        )
        .unwrap();
        let preview = preview_import(&conn, &sample_bundle(), ImportMode::Merge).unwrap();
        let warning = preview
            .warnings
            .iter()
            .find(|w| w.contains("import will abort"))
            .unwrap_or_else(|| panic!("{:?}", preview.warnings));
        // Names the actual cause and its fix (review finding: the warning
        // used to blame a missing fallback for every hold cause).
        assert!(
            warning.contains("declares no `fallback_node_type`"),
            "{warning}"
        );
        assert!(
            warning.contains("`ct ontology set --fallback <type>` with one of: person"),
            "{warning}"
        );
        assert!(apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).is_err());
    }

    /// Preview/apply parity for §2.4.5's SECOND Held trigger: a strict
    /// tier_fact that DECLARES a fallback over ZERO node_types. The resolver
    /// now holds it (`HoldReason::EmptyNodeTypes`) — the preview must warn
    /// exactly as the apply aborts (review finding: the
    /// `== GateDecision::Held` probe once reported this brain importable).
    #[test]
    fn preview_warns_on_declared_fallback_over_empty_node_types() {
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('tier_fact', 'strict', '{\"node_types\":[],\"edge_types\":[],\"fallback_node_type\":\"person\"}', 1)",
            [],
        )
        .unwrap();
        let preview = preview_import(&conn, &sample_bundle(), ImportMode::Merge).unwrap();
        let warning = preview
            .warnings
            .iter()
            .find(|w| w.contains("import will abort"))
            .unwrap_or_else(|| panic!("{:?}", preview.warnings));
        // The manifest already HAS a fallback: the warning must name the
        // empty node-type set and must not prescribe `--fallback`.
        assert!(warning.contains("declares no node types"), "{warning}");
        assert!(!warning.contains("--fallback"), "{warning}");
        assert!(apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).is_err());
    }

    #[test]
    fn extract_citations_urls_handles_uppercase_protocol() {
        let body = "# Citations\nHTTPS://example.com/path\n";
        assert_eq!(
            extract_citations_urls(body),
            vec!["HTTPS://example.com/path".to_string()]
        );
    }

    #[test]
    fn extract_citations_urls_handles_titlecase_protocol() {
        let body = "# Citations\nHttps://example.com/path\n";
        assert_eq!(
            extract_citations_urls(body),
            vec!["Https://example.com/path".to_string()]
        );
    }

    #[test]
    fn extract_citations_urls_handles_mixed_case_protocol() {
        let body = "# Citations\nhTtPs://example.com/path\n";
        assert_eq!(
            extract_citations_urls(body),
            vec!["hTtPs://example.com/path".to_string()]
        );
    }

    #[test]
    fn extract_citations_urls_tolerates_blank_line_after_heading() {
        // Standard markdown: heading, blank line, then the bullet list. The
        // pre-fix implementation broke on the blank line immediately after
        // the heading, which is the exact shape every formatter emits.
        let body =
            "Some intro text.\n\n# Citations\n\n- https://example.com/a\n- https://example.com/b\n";
        assert_eq!(
            extract_citations_urls(body),
            vec![
                "https://example.com/a".to_string(),
                "https://example.com/b".to_string(),
            ]
        );
    }

    #[test]
    fn extract_citations_urls_terminates_at_next_heading() {
        // A second `# …` heading (e.g. `# Notes`) must end the citations
        // section; URLs after it must not be captured.
        let body = "# Citations\n\n- https://example.com/a\n\n# Notes\n\n- https://example.com/b\n";
        assert_eq!(
            extract_citations_urls(body),
            vec!["https://example.com/a".to_string()]
        );
    }

    #[test]
    fn uppercase_pseudo_token_is_treated_as_legacy_and_normalized() {
        // Review round 5, finding 2: the local shape test accepted
        // `is_ascii_hexdigit` (uppercase too), so a human-edited bundle
        // carrying `librarian-<32 UPPERCASE hex>` was stored verbatim while
        // every reader classified it as a legacy document path.
        let mut conn = open_in_memory().unwrap();
        let mut bundle = sample_bundle();
        bundle.entities[0].facts[0].source_type = "librarian_inferred".into();
        bundle.entities[0].facts[0].source_ref =
            Some("librarian-ABCDEF0123456789ABCDEF0123456789".into());

        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        let stored: String = conn
            .query_row(
                "SELECT source_ref FROM llm_wiki_entries WHERE id='fact_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            stored,
            crate::db::commit::librarian_source_ref_token("fact_1"),
            "an uppercase pseudo-token must be normalized, not stored verbatim"
        );
        // The uppercase ref is not JSON, so nothing was salvageable — the drop
        // must be surfaced as a warning, not silence. (Plus the §8.4 summary
        // line, which now rides warnings — review round 1 m1.)
        assert_eq!(result.warnings.len(), 2);
    }

    #[test]
    fn engine_mangled_ref_is_tokenized_with_a_warning_not_silent_loss() {
        // Review round 5, finding 3: a pre-fix bundle whose librarian facts
        // carry engine-mangled (non-JSON) refs used to get a fresh token with
        // no evidence row, no backup, and no warning. The dropped ref must
        // now be preserved verbatim in the result warnings.
        let mut conn = open_in_memory().unwrap();
        let mut bundle = sample_bundle();
        bundle.entities[0].facts[0].source_type = "librarian_inferred".into();
        bundle.entities[0].facts[0].source_ref =
            Some("evidencechunk_id7content_hashfeedface00".into());

        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        assert_eq!(result.facts_added, 1);
        assert_eq!(
            result.warnings.len(),
            2, // the dropped-provenance warning + the §8.4 summary line (r1 m1)
            "the dropped provenance must be surfaced, not silently destroyed"
        );
        assert!(
            result.warnings[0].contains("evidencechunk_id7content_hashfeedface00"),
            "warning must preserve the dropped ref verbatim: {}",
            result.warnings[0]
        );
        let evidence_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM librarian_evidence WHERE entry_id = 'fact_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(evidence_rows, 0, "nothing was salvageable");
    }

    #[test]
    fn json_null_proposal_id_is_not_salvaged_into_an_empty_proposal_row() {
        // Review round 5, finding 10: the salvage gate accepted a JSON-null
        // proposal_id (`get().is_some()`) and then `.unwrap_or_default()`
        // wrote an evidence row keyed `proposal_id = ''` that no retraction
        // could ever match. Strict extraction must route it to a warning.
        let mut conn = open_in_memory().unwrap();
        let mut bundle = sample_bundle();
        bundle.entities[0].facts[0].source_type = "librarian_inferred".into();
        bundle.entities[0].facts[0].source_ref =
            Some(r#"{"proposal_id":null,"evidence":[]}"#.into());

        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        assert_eq!(
            result.warnings.len(),
            2, // the unsalvageable-null warning + the §8.4 summary line (r1 m1)
            "a JSON-null proposal_id is not salvageable"
        );
        let empty_pid_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM librarian_evidence WHERE proposal_id = ''",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            empty_pid_rows, 0,
            "no evidence row may be written under an empty proposal_id"
        );
    }

    /// #398 acceptance: a bundle round-tripped through DISK TEXT (export →
    /// write → read back → apply) must carry `forgotten_at` verbatim, and the
    /// gate must hide the entry whose ONLY backing event is forgotten. The
    /// survival guard keeps an entry alive if ANY live backing event exists,
    /// so the two events back DIFFERENT entries (r1-B2).
    #[test]
    fn round_trip_preserves_forgotten_at_and_gate_hides_entry() {
        // 1. Source brain: `open_in_memory()` already yields the v19 shape
        //    (`engine9_migrated_fixture` is private to connection.rs). Seed
        //    TWO live entries, both source_type NOT 'user_stated' (carve-out)
        //    and neither id == entity_id nor tier_* (carve-outs).
        let src = open_in_memory().unwrap();
        src.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_src', 'ent_src', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        for fact_id in ["fact_a", "fact_b"] {
            src.execute(
                "INSERT INTO llm_wiki_entries (
                    id, entity_id, title, body, tags, confidence, source_type,
                    created_at, updated_at
                 ) VALUES (?1, 'ent_src', 'T', 'B', '[]', 'certain', 'user_confirmed', 1, 1)",
                [fact_id],
            )
            .unwrap();
        }
        src.execute(
            "INSERT INTO llm_wiki_events
                (id, entity_id, event_type, summary, related_entry_id, created_at, forgotten_at)
             VALUES ('evt_dead', 'ent_src', 'action', 'd', 'fact_a', 1, 1732713600000)",
            [],
        )
        .unwrap();
        src.execute(
            "INSERT INTO llm_wiki_events
                (id, entity_id, event_type, summary, related_entry_id, created_at, forgotten_at)
             VALUES ('evt_live', 'ent_src', 'action', 'l', 'fact_b', 1, NULL)",
            [],
        )
        .unwrap();

        // 2. Round-trip through DISK TEXT — never build the ParsedBundle in
        //    memory (r3 minor 2): export → write_bundle → files on a temp dir
        //    → read the files back → parse → apply on a FRESH brain.
        let export = crate::db::bundle_io::load_export_entities(&src, None).unwrap();
        let files = crate::okf::bundle_write::write_bundle(&export).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let mut back = Vec::new();
        for f in &files {
            let dst = tmp.path().join(&f.path);
            if let Some(dir) = dst.parent() {
                std::fs::create_dir_all(dir).unwrap();
            }
            std::fs::write(&dst, &f.content).unwrap();
            back.push(crate::okf::types::OkfFile {
                path: f.path.clone(),
                content: std::fs::read_to_string(&dst).unwrap(),
            });
        }
        let bundle = crate::okf::bundle_read::parse_bundle(&back).unwrap();
        let mut conn = open_in_memory().unwrap();
        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();
        assert_eq!(result.events_added, 2, "both events import as new");

        // 3. Assert with the REAL gate arm formatted in (no pseudo-SQL).
        let arm = crate::wisdom_match::forgotten_entry_arm(
            &crate::wisdom_match::temporal_columns_shape_only(&conn).unwrap(),
        );
        let visible = |id: &str| -> i64 {
            let sql = format!(
                "SELECT COUNT(*) FROM llm_wiki_entries e \
                 WHERE e.id=?1 AND e.deleted_at IS NULL{arm}"
            );
            conn.query_row(&sql, params![id], |r| r.get(0)).unwrap()
        };
        assert_eq!(visible("fact_a"), 0, "fact_a's only event is forgotten");
        assert_eq!(visible("fact_b"), 1, "fact_b has a live backing event");
        let dead: Option<i64> = conn
            .query_row(
                "SELECT forgotten_at FROM llm_wiki_events WHERE id='evt_dead'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dead, Some(1732713600000), "evt_dead carries forgotten_at");
        let live: Option<i64> = conn
            .query_row(
                "SELECT forgotten_at FROM llm_wiki_events WHERE id='evt_live'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, None, "evt_live stays live");
    }

    /// r1-M3: the target already has evt_dead LIVE (imported before the
    /// forget). A later bundle says evt_dead forgotten_at = Some(ms).
    /// `event_exists` marks it dup — but the import must still tombstone the
    /// target's live row, or an already-synced brain keeps the resurrection
    /// alive forever. The reconciled dup counts only as `events_skipped` —
    /// the tombstone is intentionally silent in `ImportResult` (r3 minor 6).
    #[test]
    fn dup_import_reconciles_a_forgotten_event() {
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_a', 'ent_a', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                created_at, updated_at
             ) VALUES ('fact_x', 'ent_a', 'T', 'B', '[]', 'certain', 'user_confirmed', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_events
                (id, entity_id, event_type, summary, related_entry_id, created_at, forgotten_at)
             VALUES ('evt_dead', 'ent_a', 'action', 'd', 'fact_x', 1, NULL)",
            [],
        )
        .unwrap();

        let mut bundle = sample_bundle();
        bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_dead".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_x".into()),
            date: "1970-01-01".into(),
            forgotten_at: Some(1732713600000),
        };
        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        assert_eq!(result.events_skipped, 1, "dup counted as skipped");
        assert_eq!(result.events_added, 0, "no duplicate row inserted");
        let forgotten: Option<i64> = conn
            .query_row(
                "SELECT forgotten_at FROM llm_wiki_events WHERE id='evt_dead'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            forgotten,
            Some(1732713600000),
            "the dup's target row must be tombstoned"
        );
    }

    #[test]
    fn dup_with_colliding_id_on_another_entity_stays_live() {
        // T1.5 r1 MAJOR regression, T4 r1 m5 wording: bundle content naming an
        // event id that exists under an entity OUTSIDE the import's cluster
        // must not mutate that row — the dup tombstone is scoped to the
        // target's cluster (survivor ∪ redirect losers). Before the
        // reconciliation existed, such a dup was skipped with no mutation;
        // the tombstone must not widen that reach.
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_a', 'ent_a', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_b', 'ent_b', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        // ent_b's LIVE event carries the id the bundle will collide with.
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                created_at, updated_at
             ) VALUES ('fact_b', 'ent_b', 'T', 'B', '[]', 'certain', 'user_confirmed', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_events
                (id, entity_id, event_type, summary, related_entry_id, created_at, forgotten_at)
             VALUES ('evt_coll', 'ent_b', 'action', 'd', 'fact_b', 1, NULL)",
            [],
        )
        .unwrap();

        let mut bundle = sample_bundle();
        bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_coll".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_1".into()),
            date: "1970-01-01".into(),
            forgotten_at: Some(1732713600000),
        };
        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        assert_eq!(
            result.events_skipped, 1,
            "id-collision counted as skipped"
        );
        let forgotten: Option<i64> = conn
            .query_row(
                "SELECT forgotten_at FROM llm_wiki_events WHERE id='evt_coll'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            forgotten, None,
            "the OTHER entity's live row must stay live"
        );
    }

    #[test]
    fn dup_tombstone_reaches_merged_loser_rows() {
        // CodeRabbit CR-1: export loads the survivor's WHOLE cluster (via
        // entity_id IN, losers included), and loser rows keep
        // entity_id = loser. A forgotten dup owned by a merged-away loser
        // must be reconciled — scoping the tombstone to the survivor alone
        // would leave it live and resurrect its entry after the import.
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_s', 'ent_s', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_a', 'ent_a', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES ('ent_a', 'ent_s', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_events
                (id, entity_id, event_type, summary, related_entry_id, created_at, forgotten_at)
             VALUES ('evt_m', 'ent_a', 'action', 'd', NULL, 1, NULL)",
            [],
        )
        .unwrap();

        let mut bundle = sample_bundle();
        bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_m".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_1".into()),
            date: "1970-01-01".into(),
            forgotten_at: Some(1732713600000),
        };
        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        assert_eq!(result.events_skipped, 1, "dup counted as skipped");
        let forgotten: Option<i64> = conn
            .query_row(
                "SELECT forgotten_at FROM llm_wiki_events WHERE id='evt_m'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            forgotten,
            Some(1732713600000),
            "the merged-loser's row must be tombstoned by the import"
        );
    }

    #[test]
    fn forgotten_fresh_insert_pointing_outside_cluster_keeps_linkage_dropped() {
        // T4 r1 M1 + r2 M1/M2 regression: the survival guard is
        // entity-agnostic, so a forgotten event naming a related entry owned
        // OUTSIDE the import's cluster would HIDE that foreign entry — and
        // after a re-import the dup path would tombstone a downgraded-live
        // row right back. Fix: the tombstone is kept verbatim but the
        // LINKAGE is dropped (related_entry_id = NULL), so the event can
        // neither hide nor back any entry, and re-import dups are no-ops.
        let mut conn = open_in_memory().unwrap();
        // Import target.
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_s', 'ent_s', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        // FOREIGN entity owning the entry the bundle's event will name.
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_b', 'ent_b', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                created_at, updated_at
             ) VALUES ('fact_b', 'ent_b', 'T', 'B', '[]', 'certain', 'user_confirmed', 1, 1)",
            [],
        )
        .unwrap();

        let mut bundle = sample_bundle();
        bundle.entities[0].entity_id = "ent_s".into();
        bundle.entities[0].facts[0].id = "fact_b".into(); // id collision → skipped
        bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_c".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_b".into()), // FOREIGN entry
            date: "1970-01-01".into(),
            forgotten_at: Some(1732713600000),
        };
        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        let (forgotten, related): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT forgotten_at, related_entry_id FROM llm_wiki_events WHERE id='evt_c'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            forgotten,
            Some(1732713600000),
            "the tombstone is applied verbatim (downgrade-to-live was rejected in T4 r2)"
        );
        assert_eq!(
            related, None,
            "the linkage to a foreign entry must be dropped"
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("evt_c") && w.contains("linkage dropped")),
            "the linkage drop must surface as an import warning"
        );
        // The foreign entry must stay VISIBLE through the real gate arm.
        let arm = crate::wisdom_match::FORGOTTEN_ENTRY_ARM_SQL;
        let visible: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM llm_wiki_entries e \
                     WHERE e.id='fact_b' AND e.deleted_at IS NULL{arm}"
                ),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(visible, 1, "fact_b must stay visible through the gate arm");
        // T4 r2 M1: a SECOND import of the same bundle must not change any of
        // the above — the dup is a tombstone-wins no-op.
        let result2 = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();
        assert_eq!(result2.events_skipped, 1, "second import sees the dup");
        let (forgotten2, related2): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT forgotten_at, related_entry_id FROM llm_wiki_events WHERE id='evt_c'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(forgotten2, Some(1732713600000));
        assert_eq!(related2, None);
        let visible2: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM llm_wiki_entries e \
                     WHERE e.id='fact_b' AND e.deleted_at IS NULL{arm}"
                ),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            visible2, 1,
            "the re-import must not re-hide the foreign entry"
        );
    }

    #[test]
    fn forgotten_fresh_insert_related_to_loser_owned_entry_keeps_linkage() {
        // T4 r2 m3: the keep branch must cover LOSER-owned entries (a
        // regression to survivor-only scoping would wrongly drop the linkage
        // for rows export itself includes in the cluster).
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_s', 'ent_s', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_a', 'ent_a', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES ('ent_a', 'ent_s', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                created_at, updated_at
             ) VALUES ('fact_la', 'ent_a', 'T', 'B', '[]', 'certain', 'user_confirmed', 1, 1)",
            [],
        )
        .unwrap();

        let mut bundle = sample_bundle();
        bundle.entities[0].entity_id = "ent_s".into();
        bundle.entities[0].facts[0].id = "fact_new".into();
        bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_la".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_la".into()), // loser-owned, IN cluster
            date: "1970-01-01".into(),
            forgotten_at: Some(1732713600000),
        };
        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        let (forgotten, related): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT forgotten_at, related_entry_id FROM llm_wiki_events WHERE id='evt_la'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(forgotten, Some(1732713600000));
        assert_eq!(
            related.as_deref(),
            Some("fact_la"),
            "a cluster-owned (loser) entry keeps its linkage"
        );
        assert!(
            !result
                .warnings
                .iter()
                .any(|w| w.contains("evt_la") && w.contains("linkage dropped")),
            "no warning for in-cluster linkage"
        );
    }

    #[test]
    fn forgotten_fresh_insert_related_to_missing_entry_drops_linkage() {
        // T4 r2 m3 (None-branch): an id that exists nowhere yet could name an
        // entry imported LATER — the linkage is dropped, and the warning
        // distinguishes it from the outside-cluster case. (A TASK-related id
        // is kept — task ids cannot hide entries.)
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_s', 'ent_s', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();

        let mut bundle = sample_bundle();
        bundle.entities[0].entity_id = "ent_s".into();
        bundle.entities[0].facts[0].id = "fact_new".into();
        bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_m".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_nowhere".into()),
            date: "1970-01-01".into(),
            forgotten_at: Some(1732713600000),
        };
        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        let related: Option<String> = conn
            .query_row(
                "SELECT related_entry_id FROM llm_wiki_events WHERE id='evt_m'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(related, None, "missing-entry linkage must be dropped");
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("evt_m") && w.contains("linkage dropped")),
            "the linkage drop must surface as an import warning"
        );
    }

    #[test]
    fn tombstone_wins_incoming_live_dup_stays_forgotten() {
        // T4 r1 m4: an incoming LIVE dup must not resurrect a target row that
        // is already forgotten (spec r1-M3 tombstone-wins).
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_a', 'ent_a', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                created_at, updated_at
             ) VALUES ('fact_x', 'ent_a', 'T', 'B', '[]', 'certain', 'user_confirmed', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_events
                (id, entity_id, event_type, summary, related_entry_id, created_at, forgotten_at)
             VALUES ('evt_dead', 'ent_a', 'action', 'd', 'fact_x', 1, 1111111111111)",
            [],
        )
        .unwrap();

        let mut bundle = sample_bundle();
        bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_dead".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_x".into()),
            date: "1970-01-01".into(),
            forgotten_at: None, // incoming event is LIVE
        };
        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        assert_eq!(result.events_skipped, 1);
        let forgotten: Option<i64> = conn
            .query_row(
                "SELECT forgotten_at FROM llm_wiki_events WHERE id='evt_dead'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            forgotten,
            Some(1111111111111),
            "a live dup must never resurrect an already-forgotten row"
        );
    }

    #[test]
    fn tombstone_keeps_original_timestamp_on_conflicting_ms() {
        // T4 r1 m4: tombstone-wins keeps the TARGET's original timestamp — an
        // incoming dup with a different forgotten_at never rewrites it.
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_a', 'ent_a', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                created_at, updated_at
             ) VALUES ('fact_x', 'ent_a', 'T', 'B', '[]', 'certain', 'user_confirmed', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_events
                (id, entity_id, event_type, summary, related_entry_id, created_at, forgotten_at)
             VALUES ('evt_dead', 'ent_a', 'action', 'd', 'fact_x', 1, 1111111111111)",
            [],
        )
        .unwrap();

        let mut bundle = sample_bundle();
        bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_dead".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_x".into()),
            date: "1970-01-01".into(),
            forgotten_at: Some(2222222222222), // conflicting ms
        };
        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        assert_eq!(result.events_skipped, 1);
        let forgotten: Option<i64> = conn
            .query_row(
                "SELECT forgotten_at FROM llm_wiki_events WHERE id='evt_dead'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            forgotten,
            Some(1111111111111),
            "the target's original tombstone timestamp must be kept"
        );
    }

    #[test]
    fn live_fresh_insert_foreign_linkage_dropped_and_two_imports_never_flip_visibility() {
        // T4 r3 M1+M2 regression: the linkage gate covers EVERY fresh insert,
        // live or forgotten. (a) A LIVE event naming a foreign entry must not
        // resurrect (un-hide) it (M2). (b) A later forgotten import of the
        // same event must not hide it through the dup path (M1's
        // live-then-forgotten route).
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_s', 'ent_s', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_b', 'ent_b', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                created_at, updated_at
             ) VALUES ('fact_b', 'ent_b', 'T', 'B', '[]', 'certain', 'user_confirmed', 1, 1)",
            [],
        )
        .unwrap();
        // fact_b is LOCALLY hidden: its only backing event is forgotten.
        conn.execute(
            "INSERT INTO llm_wiki_events
                (id, entity_id, event_type, summary, related_entry_id, created_at, forgotten_at)
             VALUES ('evt_hidden', 'ent_b', 'action', 'd', 'fact_b', 1, 1111111111111)",
            [],
        )
        .unwrap();
        let arm = crate::wisdom_match::FORGOTTEN_ENTRY_ARM_SQL;
        let visible = |conn: &Connection| -> i64 {
            conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM llm_wiki_entries e \
                     WHERE e.id='fact_b' AND e.deleted_at IS NULL{arm}"
                ),
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(visible(&conn), 0, "fixture: fact_b starts hidden");

        let mut bundle = sample_bundle();
        bundle.entities[0].entity_id = "ent_s".into();
        bundle.entities[0].facts[0].id = "fact_new".into();
        bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_c".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_b".into()), // FOREIGN entry
            date: "1970-01-01".into(),
            forgotten_at: None, // LIVE fresh insert
        };
        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        let related: Option<String> = conn
            .query_row(
                "SELECT related_entry_id FROM llm_wiki_events WHERE id='evt_c'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            related, None,
            "a live event naming a foreign entry must lose its linkage too (M2)"
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("evt_c") && w.contains("linkage dropped")),
            "the live-insert linkage drop must surface as an import warning"
        );
        assert_eq!(
            visible(&conn),
            0,
            "the live insert must not resurrect (un-hide) the foreign entry (M2)"
        );

        // M1: a later import where evt_c is FORGOTTEN must not hide fact_b
        // through the dup path either — the row has no linkage to hide with.
        let mut bundle2 = sample_bundle();
        bundle2.entities[0].entity_id = "ent_s".into();
        bundle2.entities[0].facts[0].id = "fact_new2".into();
        bundle2.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_c".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_b".into()),
            date: "1970-01-01".into(),
            forgotten_at: Some(1732713600000),
        };
        let result2 = apply_import(&mut conn, &bundle2, ImportMode::Merge).unwrap();
        assert_eq!(result2.events_skipped, 1, "second import sees the dup");
        assert_eq!(
            visible(&conn),
            0,
            "fact_b stays hidden by its own event — and only by its own event; \
             the dup tombstone must not add foreign reach"
        );
    }

    #[test]
    fn task_related_forgotten_linkage_kept() {
        // T4 r3 m2: a task-related tombstone cannot hide anything (the guard
        // joins llm_wiki_entries; task ids are a distinct namespace) — its
        // linkage is kept, with no warning.
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_s', 'ent_s', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_tasks (id, entity_id, description, created_at, updated_at)
             VALUES ('task_t', 'ent_s', 'd', 1, 1)",
            [],
        )
        .unwrap();

        let mut bundle = sample_bundle();
        bundle.entities[0].entity_id = "ent_s".into();
        bundle.entities[0].facts[0].id = "fact_new".into();
        bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_t".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("task_t".into()),
            date: "1970-01-01".into(),
            forgotten_at: Some(1732713600000),
        };
        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        let related: Option<String> = conn
            .query_row(
                "SELECT related_entry_id FROM llm_wiki_events WHERE id='evt_t'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            related.as_deref(),
            Some("task_t"),
            "task-related linkage is kept — it cannot hide any entry"
        );
        assert!(
            !result
                .warnings
                .iter()
                .any(|w| w.contains("evt_t") && w.contains("linkage dropped")),
            "no warning for task-related linkage"
        );
    }

    #[test]
    fn clone_mode_foreign_linkage_dropped() {
        // T4 r3 m2 + r4 m7: Clone mode runs the same gate — defence in depth
        // for the public apply_import contract (bundle_read maps every
        // related id through id_map in Clone mode, so a production clone
        // cannot produce this state; the public function still must not
        // trust it).
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_b', 'ent_b', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                created_at, updated_at
             ) VALUES ('fact_b', 'ent_b', 'T', 'B', '[]', 'certain', 'user_confirmed', 1, 1)",
            [],
        )
        .unwrap();

        let mut bundle = sample_bundle();
        bundle.entities[0].facts[0].id = "fact_own".into();
        bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_k".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_b".into()), // foreign, unmapped in Clone
            date: "1970-01-01".into(),
            forgotten_at: Some(1732713600000),
        };
        let result = apply_import(&mut conn, &bundle, ImportMode::Clone).unwrap();
        // Clone mode re-keys ids: the event's id is generated, so find the
        // row via the cloned entity (the clone carries the bundle display
        // name; ent_b is the only other entity). Exclude the 'imported' audit
        // event — it shares the entity and never carries the flag.
        let clone_id: String = conn
            .query_row(
                "SELECT id FROM curated_entities WHERE name = 'Project X'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let (related, forgotten): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT related_entry_id, forgotten_at FROM llm_wiki_events \
                 WHERE entity_id=?1 AND event_type <> 'imported'",
                [&clone_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(related, None, "clone-mode foreign linkage must be dropped");
        assert_eq!(
            forgotten,
            Some(1732713600000),
            "the tombstone itself is still applied verbatim"
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("linkage dropped")),
            "the clone-mode linkage drop must surface as an import warning \
             (the event id is re-keyed in Clone, so match on the action, not the id)"
        );
    }

    #[test]
    fn two_import_route_cannot_hide_a_visible_foreign_entry() {
        // T4 r4 m1: fact_b starts VISIBLE (no local events). Import 1 brings a
        // LIVE event naming it (linkage dropped). Import 2 brings the same
        // event FORGOTTEN — the dup tombstone must REFUSE the row (its
        // linkage is NULL... and even legacy foreign linkage is refused by
        // the dup UPDATE's linkage guard), so fact_b stays visible and evt_c
        // stays live.
        let mut conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_s', 'ent_s', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_b', 'ent_b', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                created_at, updated_at
             ) VALUES ('fact_b', 'ent_b', 'T', 'B', '[]', 'certain', 'user_confirmed', 1, 1)",
            [],
        )
        .unwrap();
        let arm = crate::wisdom_match::FORGOTTEN_ENTRY_ARM_SQL;
        let visible = |conn: &Connection| -> i64 {
            conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM llm_wiki_entries e \
                     WHERE e.id='fact_b' AND e.deleted_at IS NULL{arm}"
                ),
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(visible(&conn), 1, "fixture: fact_b starts visible");

        let mut live_bundle = sample_bundle();
        live_bundle.entities[0].entity_id = "ent_s".into();
        live_bundle.entities[0].facts[0].id = "fact_new".into();
        live_bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_c".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_b".into()), // FOREIGN entry
            date: "1970-01-01".into(),
            forgotten_at: None, // live
        };
        apply_import(&mut conn, &live_bundle, ImportMode::Merge).unwrap();

        let mut forgotten_bundle = sample_bundle();
        forgotten_bundle.entities[0].entity_id = "ent_s".into();
        forgotten_bundle.entities[0].facts[0].id = "fact_new".into();
        forgotten_bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_c".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_b".into()),
            date: "1970-01-01".into(),
            forgotten_at: Some(1732713600000),
        };
        let result2 = apply_import(&mut conn, &forgotten_bundle, ImportMode::Merge).unwrap();
        assert_eq!(result2.events_skipped, 1, "second import sees the dup");

        let (forgotten, related): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT forgotten_at, related_entry_id FROM llm_wiki_events WHERE id='evt_c'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            forgotten, Some(1732713600000),
            "the dup tombstone applies to the linkage-less row — harmless: \
             with related_entry_id NULL it cannot hide fact_b (asserted below)"
        );
        assert_eq!(related, None, "import 1 already dropped the linkage");
        assert_eq!(
            visible(&conn),
            1,
            "fact_b must stay visible through the whole two-import route"
        );

        // T4 r4 M1 legacy route: a row that ALREADY carries foreign linkage
        // (minted by a pre-gate build) must refuse the tombstone — otherwise
        // the same hide is reachable through legacy rows.
        conn.execute(
            "INSERT INTO llm_wiki_events
                (id, entity_id, event_type, summary, related_entry_id, created_at, forgotten_at)
             VALUES ('evt_legacy', 'ent_s', 'action', 'd', 'fact_b', 1, NULL)",
            [],
        )
        .unwrap();
        let mut legacy_bundle = sample_bundle();
        legacy_bundle.entities[0].entity_id = "ent_s".into();
        legacy_bundle.entities[0].facts[0].id = "fact_new3".into();
        legacy_bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_legacy".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_b".into()),
            date: "1970-01-01".into(),
            forgotten_at: Some(1732713600000),
        };
        let result3 = apply_import(&mut conn, &legacy_bundle, ImportMode::Merge).unwrap();
        assert_eq!(result3.events_skipped, 1, "legacy row seen as a dup");
        let legacy_forgotten: Option<i64> = conn
            .query_row(
                "SELECT forgotten_at FROM llm_wiki_events WHERE id='evt_legacy'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            legacy_forgotten, None,
            "the dup tombstone must refuse a row whose linkage names a foreign entry"
        );
        assert_eq!(
            visible(&conn),
            1,
            "fact_b stays visible — the legacy route is closed"
        );
    }

    #[test]
    fn pre_v19_shape_live_foreign_linkage_dropped() {
        // T4 r4 M1 + m2: the linkage gate is UNCONDITIONAL — it runs on
        // pre-v19 shapes too, so a pre-gate import cannot mint the legacy
        // foreign linkage that a later column-addition + dup tombstone would
        // turn into a foreign hide.
        let mut conn = open_in_memory().unwrap();
        crate::db::test_support::drop_forgotten_column(&conn); // pre-v19 shape
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_b', 'ent_b', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                created_at, updated_at
             ) VALUES ('fact_b', 'ent_b', 'T', 'B', '[]', 'certain', 'user_confirmed', 1, 1)",
            [],
        )
        .unwrap();

        let mut bundle = sample_bundle();
        bundle.entities[0].facts[0].id = "fact_own".into();
        bundle.entities[0].events[0] = ParsedEvent {
            event_id: Some("evt_p".into()),
            event_type: "action".into(),
            summary: "d".into(),
            related_entry_id: Some("fact_b".into()), // FOREIGN entry
            date: "1970-01-01".into(),
            forgotten_at: None,
        };
        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();

        let related: Option<String> = conn
            .query_row(
                "SELECT related_entry_id FROM llm_wiki_events WHERE id='evt_p'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            related, None,
            "the gate runs on pre-v19 shapes — no legacy linkage may be minted"
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("evt_p") && w.contains("linkage dropped")),
            "the pre-v19 linkage drop must surface as an import warning"
        );
    }

    /// r1-B3 / r2-B2: a pre-v19 target (no `forgotten_at` column) must import
    /// without error, twice — the second pass is a dup and must skip cleanly
    /// instead of aborting prepare with "no such column" and rolling back.
    #[test]
    fn pre_v19_target_imports_without_forgotten_column() {
        let mut conn = open_in_memory().unwrap();
        crate::db::test_support::drop_forgotten_column(&conn);

        let mut bundle = sample_bundle();
        bundle.entities[0].events[0].forgotten_at = Some(1732713600000);

        let result = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();
        assert_eq!(result.events_added, 1, "first import adds the event");

        // r2-B2: the SAME bundle again — now a dup (event_exists matches by
        // id) with forgotten_at still Some(ms). Must not abort or roll back.
        let second = apply_import(&mut conn, &bundle, ImportMode::Merge).unwrap();
        assert_eq!(second.events_skipped, 1, "dup skipped on re-import");
        assert_eq!(second.events_added, 0);
    }

    /// A legacy bundle (no `forgotten:` key) imports live on a v19 brain.
    #[test]
    fn legacy_bundle_without_key_imports_live() {
        let mut conn = open_in_memory().unwrap();
        let result = apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        assert_eq!(result.events_added, 1);
        let forgotten: Option<i64> = conn
            .query_row(
                "SELECT forgotten_at FROM llm_wiki_events WHERE id='evt_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(forgotten, None, "a legacy event must land live");
    }
}
