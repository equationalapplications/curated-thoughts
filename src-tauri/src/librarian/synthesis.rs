//! OKF-native librarian synthesis: structured JSON proposals into `curated_proposals`.

use crate::db::commit::{resolve_proposal, ResolveOptions};
use crate::db::proposals::{
    get_proposal_detail, insert_proposal, ItemDecision, ItemDecisionKind, NewProposal,
    NewProposalItem, NewProposalSource, ProposalKind, ProposalSourceRole, StoredEvidenceChunk,
};
use crate::inference::config::{read_config, GenerationProviderKind, LlmConfig};
use crate::librarian::{assemble_librarian_context, build_structural_context, ChunkRow};
use crate::search::{bytes_to_f32, cosine_similarity};
use anyhow::{Context, Result};
use rand::RngCore;
use rusqlite::{params, Connection};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const MAX_CANDIDATES: usize = 8;
const MAX_FACTS_PER_CANDIDATE: usize = 5;
const CONTEXT_BYTE_LIMIT: usize = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynthesisMode {
    Summarize,
    Synthesize,
}

#[derive(Debug, Clone)]
struct NumberedChunk {
    label: String,
    chunk_id: i64,
    doc_id: i64,
    text: String,
    start_line: i64,
    end_line: i64,
    /// Stable SHA-256 first-16-bytes hex. Carried into evidence so
    /// pending proposals survive chunk replacement — `chunk_id` may
    /// orphan but `content_hash` resolves through the Phase 9 index.
    content_hash: String,
}

#[derive(Debug, Clone)]
struct CandidateFact {
    id: String,
    body: String,
}

#[derive(Debug, Clone)]
struct CandidateEntity {
    id: String,
    name: String,
    entity_type: String,
    summary_snippet: String,
    facts: Vec<CandidateFact>,
}

#[derive(Debug, Deserialize)]
struct LlmSynthesisOutput {
    proposals: Vec<LlmProposal>,
}

#[derive(Debug, Deserialize)]
struct LlmProposal {
    target: LlmTarget,
    reasoning: String,
    #[serde(default)]
    summary_update: Option<String>,
    #[serde(default)]
    facts: Vec<LlmFact>,
    #[serde(default)]
    edges: Vec<LlmEdge>,
    #[serde(default)]
    tasks: Vec<LlmTask>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LlmTarget {
    Existing { existing_id: String },
    New { new: LlmNewEntity },
}

#[derive(Debug, Deserialize)]
struct LlmNewEntity {
    name: String,
    #[serde(rename = "type")]
    entity_type: String,
}

#[derive(Debug, Deserialize)]
struct LlmFact {
    op: String,
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    confidence: Option<String>,
    #[serde(default)]
    evidence: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct LlmEdge {
    source: serde_json::Value,
    target: serde_json::Value,
    edge_type: String,
}

#[derive(Debug, Deserialize)]
struct LlmTask {
    description: String,
    #[serde(default)]
    evidence: Vec<String>,
}

#[derive(Debug, Clone)]
struct ValidatedProposal {
    proposal: NewProposal,
    items: Vec<NewProposalItem>,
    sources: Vec<NewProposalSource>,
}

pub(crate) trait LlmCompleter: Send + Sync {
    fn complete(&self, system: &str, user: &str) -> Result<String>;
}

struct HttpLlmCompleter {
    endpoint_url: String,
    api_key: Option<String>,
    model_name: String,
}

impl LlmCompleter for HttpLlmCompleter {
    fn complete(&self, system: &str, user: &str) -> Result<String> {
        let client = reqwest::blocking::Client::new();
        let mut req = client.post(&self.endpoint_url).json(&serde_json::json!({
            "model": self.model_name,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user }
            ],
            "stream": false,
        }));
        if let Some(key) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let resp = req.send()?;
        let body: serde_json::Value = resp.json()?;
        body["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
            .context("missing content in /v1/chat/completions response")
    }
}

fn generate_id(prefix: &str) -> String {
    let mut bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{prefix}{}", hex::encode(bytes))
}

fn choose_sidecar_model_name(llm_config: &LlmConfig, fallback_model: &str) -> String {
    llm_config
        .generation
        .model_name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            llm_config
                .generation
                .model_path
                .as_deref()
                .and_then(|path| {
                    Path::new(path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name.to_string())
                })
        })
        .unwrap_or_else(|| fallback_model.to_string())
}

fn build_llm_completer(model: &str) -> Result<Option<Box<dyn LlmCompleter>>> {
    let brain_dir_str = crate::get_brain_dir_inner();
    let brain_path = Path::new(&brain_dir_str);
    let llm_config = read_config(brain_path);
    let completer: Box<dyn LlmCompleter> = match &llm_config.generation.provider {
        GenerationProviderKind::Unconfigured => return Ok(None),
        GenerationProviderKind::Sidecar => {
            let base = std::env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434".to_string());
            let base = base.trim_end_matches('/');
            Box::new(HttpLlmCompleter {
                endpoint_url: format!("{base}/v1/chat/completions"),
                api_key: None,
                model_name: choose_sidecar_model_name(&llm_config, model),
            })
        }
        GenerationProviderKind::External => {
            let base = llm_config
                .generation
                .external_url
                .clone()
                .unwrap_or_default();
            let base = base.trim_end_matches('/');
            let base = base.strip_suffix("/v1").unwrap_or(base);
            Box::new(HttpLlmCompleter {
                endpoint_url: format!("{base}/v1/chat/completions"),
                api_key: llm_config.generation.api_key.clone(),
                model_name: llm_config
                    .generation
                    .model_name
                    .clone()
                    .unwrap_or_else(|| model.to_string()),
            })
        }
    };
    Ok(Some(completer))
}

pub(crate) fn strip_json_fences(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_open = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```JSON"))
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .trim();
    without_open
        .strip_suffix("```")
        .unwrap_or(without_open)
        .trim()
        .to_string()
}

fn load_numbered_chunks(
    conn: &Connection,
    source_chunks: &[ChunkRow],
) -> Result<Vec<NumberedChunk>> {
    let mut out = Vec::new();
    let mut seen_chunk_ids = HashSet::new();

    for chunk in source_chunks {
        if !seen_chunk_ids.insert(chunk.id) {
            continue;
        }
        let (doc_id, content_hash): (i64, String) = conn.query_row(
            "SELECT doc_id, content_hash FROM chunks WHERE id = ?1",
            [chunk.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let label = format!("C{}", out.len() + 1);
        out.push(NumberedChunk {
            label,
            chunk_id: chunk.id,
            doc_id,
            text: chunk.text.clone(),
            start_line: chunk.start_line,
            end_line: chunk.end_line,
            content_hash,
        });
    }

    let structural = build_structural_context(conn, source_chunks);
    if structural.is_empty() {
        return Ok(out);
    }

    let source_id_set: HashSet<i64> = source_chunks.iter().map(|c| c.id).collect();
    let entity_id = source_chunks
        .first()
        .map(|c| c.entity_id.as_str())
        .unwrap_or("");
    if !entity_id.is_empty() {
        let mut neighbor_ids = Vec::new();
        for chunk_id in source_chunks.iter().map(|c| c.id) {
            if let Ok(neighbors) = crate::graph::get_both(conn, chunk_id, entity_id, 1) {
                for n in neighbors {
                    if !source_id_set.contains(&n.chunk_id) {
                        neighbor_ids.push(n.chunk_id);
                    }
                }
            }
        }
        neighbor_ids.sort_unstable();
        neighbor_ids.dedup();
        neighbor_ids.truncate(5);

        for chunk_id in neighbor_ids {
            if !seen_chunk_ids.insert(chunk_id) {
                continue;
            }
            let row = conn.query_row(
                "SELECT doc_id, chunk_text, start_line, end_line, content_hash FROM chunks WHERE id = ?1",
                [chunk_id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            );
            if let Ok((doc_id, text, start_line, end_line, content_hash)) = row {
                let label = format!("C{}", out.len() + 1);
                out.push(NumberedChunk {
                    label,
                    chunk_id,
                    doc_id,
                    text,
                    start_line,
                    end_line,
                    content_hash,
                });
            }
        }
    }

    Ok(out)
}

fn mean_chunk_embedding(conn: &Connection, chunk_ids: &[i64]) -> Result<Option<Vec<f32>>> {
    if chunk_ids.is_empty() {
        return Ok(None);
    }
    let placeholders = chunk_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("SELECT e.vector FROM embeddings e WHERE e.chunk_id IN ({placeholders})");
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::ToSql> = chunk_ids
        .iter()
        .map(|id| id as &dyn rusqlite::ToSql)
        .collect();
    let mut rows = stmt.query(params.as_slice())?;
    let mut vecs = Vec::new();
    while let Some(row) = rows.next()? {
        let bytes: Vec<u8> = row.get(0)?;
        vecs.push(bytes_to_f32(&bytes));
    }
    if vecs.is_empty() {
        return Ok(None);
    }
    let dim = vecs[0].len();
    let mut avg = vec![0.0_f32; dim];
    for v in &vecs {
        if v.len() != dim {
            continue;
        }
        for (a, b) in avg.iter_mut().zip(v) {
            *a += b;
        }
    }
    let n = vecs.len() as f32;
    avg.iter_mut().for_each(|x| *x /= n);
    Ok(Some(avg))
}

fn name_match_entities(
    conn: &Connection,
    source_path: &str,
    chunk_text: &str,
) -> Result<Vec<String>> {
    let stem = Path::new(source_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mut stmt =
        conn.prepare("SELECT id, name FROM curated_entities WHERE deleted_at IS NULL")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let haystack = format!("{chunk_text}\n{stem}").to_lowercase();
    let mut matched = Vec::new();
    for row in rows {
        let (id, name) = row?;
        let name_lower = name.to_lowercase();
        if name_lower.is_empty() {
            continue;
        }
        if haystack.contains(&name_lower)
            || (!stem.is_empty() && (name_lower.contains(&stem) || stem.contains(&name_lower)))
        {
            matched.push(id);
        }
    }
    Ok(matched)
}

fn load_candidate_entities(
    conn: &Connection,
    source_path: &str,
    source_chunks: &[ChunkRow],
    mean_embedding: Option<&[f32]>,
) -> Result<Vec<CandidateEntity>> {
    let chunk_text: String = source_chunks.iter().map(|c| c.text.as_str()).collect();
    let name_hits = name_match_entities(conn, source_path, &chunk_text)?;
    let mut scores: HashMap<String, f32> = HashMap::new();

    if let Some(query) = mean_embedding {
        let mut stmt = conn.prepare(
            "SELECT id, summary_embedding FROM curated_entities
             WHERE deleted_at IS NULL AND summary_embedding IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        for row in rows {
            let (id, bytes) = row?;
            let vec = bytes_to_f32(&bytes);
            if vec.len() == query.len() {
                scores.insert(id, cosine_similarity(query, &vec));
            }
        }
    }

    for id in name_hits {
        scores.entry(id).or_insert(0.5);
    }

    let mut ranked: Vec<(String, f32)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.truncate(MAX_CANDIDATES);

    let mut out = Vec::new();
    for (entity_id, _) in ranked {
        let row = conn.query_row(
            "SELECT name, entity_type, summary FROM curated_entities
             WHERE id = ?1 AND deleted_at IS NULL",
            [&entity_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        );
        if let Ok((name, entity_type, summary)) = row {
            let snippet: String = summary.chars().take(200).collect();
            let mut fact_stmt = conn.prepare(
                "SELECT id, body FROM llm_wiki_entries
                 WHERE entity_id = ?1 AND deleted_at IS NULL
                 ORDER BY updated_at DESC LIMIT ?2",
            )?;
            let facts = fact_stmt
                .query_map(params![entity_id, MAX_FACTS_PER_CANDIDATE], |r| {
                    Ok(CandidateFact {
                        id: r.get(0)?,
                        body: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            out.push(CandidateEntity {
                id: entity_id,
                name,
                entity_type,
                summary_snippet: snippet,
                facts,
            });
        }
    }
    Ok(out)
}

fn format_candidates_section(candidates: &[CandidateEntity]) -> String {
    if candidates.is_empty() {
        return "CANDIDATE ENTITIES: none — propose a new entity if warranted.\n".to_string();
    }
    let mut section = String::from(
        "CANDIDATE ENTITIES — copy existing_id and fact target_id values verbatim for updates/archives:\n",
    );
    for c in candidates {
        section.push_str(&format!(
            "- existing_id: {}, name: {}, type: {}, summary: {}\n",
            c.id, c.name, c.entity_type, c.summary_snippet
        ));
        for fact in &c.facts {
            let body_snip: String = fact.body.chars().take(120).collect();
            section.push_str(&format!("  - fact_id: {}, body: {}\n", fact.id, body_snip));
        }
    }
    section.push('\n');
    section
}

fn assemble_numbered_context(chunks: &[ChunkRow], numbered: &[NumberedChunk]) -> String {
    let mut body = assemble_librarian_context(chunks);
    for nc in numbered {
        let needle = nc.text.as_str();
        if let Some(pos) = body.find(needle) {
            let insert_at = body[..pos].rfind('\n').map(|i| i + 1).unwrap_or(pos);
            body.insert_str(insert_at, &format!("[{}] ", nc.label));
        }
    }
    body
}

fn build_system_prompt(mode: SynthesisMode) -> &'static str {
    match mode {
        SynthesisMode::Summarize => {
            "You are a knowledge librarian. Analyze the document and emit structured JSON proposals for entity summary updates only. \
             Output ONLY valid JSON matching the schema — no markdown fences, no commentary.\n\n\
             Schema:\n\
             {\"proposals\":[{\"target\":{\"existing_id\":\"...\"}|{\"new\":{\"name\":\"...\",\"type\":\"person|project|concept|...\"}},\
             \"reasoning\":\"one paragraph why\",\"summary_update\":\"full replacement prose or null\",\"facts\":[],\"edges\":[],\"tasks\":[]}]}\n\n\
             Rules:\n\
             - Chunks are labeled [C1]..[Cn]; cite them in evidence arrays as \"C3\", \"C7\", etc.\n\
             - For updates/archives, target_id must be a fact_id from CANDIDATE ENTITIES.\n\
             - existing_id must be from CANDIDATE ENTITIES.\n\
             - In summarize mode: only summary_update — leave facts, edges, tasks as empty arrays.\n\
             - Contradictions with existing facts: propose fact_update or fact_archive+fact_add with reasoning (not in summarize mode)."
        }
        SynthesisMode::Synthesize => {
            "You are a knowledge librarian. Analyze the document and emit structured JSON proposals for facts, edges, tasks, and optional summary updates. \
             Output ONLY valid JSON matching the schema — no markdown fences, no commentary.\n\n\
             Schema:\n\
             {\"proposals\":[{\"target\":{\"existing_id\":\"...\"}|{\"new\":{\"name\":\"...\",\"type\":\"person|project|concept|...\"}},\
             \"reasoning\":\"one paragraph why\",\"summary_update\":\"full replacement prose or null\",\
             \"facts\":[{\"op\":\"add|update|archive\",\"target_id\":null,\"body\":\"...\",\"tags\":[],\"confidence\":\"certain|inferred\",\"evidence\":[\"C3\"]}],\
             \"edges\":[{\"source\":\"self\"|{\"existing_id\":\"...\"}|{\"new_name\":\"...\"},\"target\":\"self\"|{\"existing_id\":\"...\"}|{\"new_name\":\"...\"},\"edge_type\":\"...\"}],\
             \"tasks\":[{\"description\":\"...\",\"evidence\":[\"C2\"]}]}]}\n\n\
             Rules:\n\
             - Chunks are labeled [C1]..[Cn]; cite them in evidence arrays.\n\
             - For fact update/archive, target_id must be a fact_id from CANDIDATE ENTITIES.\n\
             - existing_id must be from CANDIDATE ENTITIES.\n\
             - Edge endpoints: \"self\" = this proposal's target entity; new_name refs sibling proposals in the same run.\n\
             - Do not modify ANCHOR TRUTH chunks; contradictions become fact_update or fact_archive proposals with reasoning."
        }
    }
}

fn truncate_context(text: &str) -> String {
    let byte_limit = text
        .char_indices()
        .nth(CONTEXT_BYTE_LIMIT)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    text[..byte_limit].to_string()
}

fn resolve_evidence(
    refs: &[String],
    chunk_by_label: &HashMap<String, &NumberedChunk>,
) -> Vec<StoredEvidenceChunk> {
    let mut out = Vec::new();
    for label in refs {
        let normalized = label.trim().trim_start_matches('[').trim_end_matches(']');
        if let Some(chunk) = chunk_by_label.get(normalized) {
            out.push(StoredEvidenceChunk {
                chunk_id: Some(chunk.chunk_id),
                content_hash: chunk.content_hash.clone(),
                quote: chunk.text.clone(),
                start_line: Some(chunk.start_line as i32),
                end_line: Some(chunk.end_line as i32),
                source_kind: None,
            });
        }
    }
    out
}

fn collect_sources(
    trigger_doc_id: i64,
    items: &[NewProposalItem],
    chunk_doc_ids: &HashMap<i64, i64>,
) -> Vec<NewProposalSource> {
    let mut sources = vec![NewProposalSource {
        doc_id: trigger_doc_id,
        role: ProposalSourceRole::Trigger,
    }];
    let mut seen = HashSet::from([trigger_doc_id]);
    for item in items {
        for ev in &item.evidence {
            if let Some(chunk_id) = ev.chunk_id {
                if let Some(&doc_id) = chunk_doc_ids.get(&chunk_id) {
                    if seen.insert(doc_id) {
                        sources.push(NewProposalSource {
                            doc_id,
                            role: ProposalSourceRole::Evidence,
                        });
                    }
                }
            }
        }
    }
    sources
}

fn validate_llm_output(
    raw: &LlmSynthesisOutput,
    mode: SynthesisMode,
    candidates: &[CandidateEntity],
    numbered: &[NumberedChunk],
    model: &str,
    trigger_doc_id: i64,
) -> Result<Vec<ValidatedProposal>> {
    let candidate_ids: HashSet<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
    let injected_fact_ids: HashSet<&str> = candidates
        .iter()
        .flat_map(|c| c.facts.iter().map(|f| f.id.as_str()))
        .collect();
    let chunk_by_label: HashMap<String, &NumberedChunk> =
        numbered.iter().map(|c| (c.label.clone(), c)).collect();
    let chunk_doc_ids: HashMap<i64, i64> =
        numbered.iter().map(|c| (c.chunk_id, c.doc_id)).collect();

    let mut validated = Vec::new();
    for llm_prop in &raw.proposals {
        let (kind, entity_id, proposed_name, proposed_type) = match &llm_prop.target {
            LlmTarget::Existing { existing_id } => {
                if !candidate_ids.contains(existing_id.as_str()) {
                    continue;
                }
                (
                    ProposalKind::UpdateEntity,
                    Some(existing_id.clone()),
                    None,
                    None,
                )
            }
            LlmTarget::New { new } => (
                ProposalKind::NewEntity,
                None,
                Some(new.name.clone()),
                Some(new.entity_type.clone()),
            ),
        };

        let mut items = Vec::new();

        if let Some(summary) = llm_prop
            .summary_update
            .as_ref()
            .filter(|s| !s.trim().is_empty())
        {
            items.push(NewProposalItem {
                id: generate_id("item_"),
                item_type: "summary_update".into(),
                target_id: None,
                payload: serde_json::json!({ "summary": summary }),
                evidence: Vec::new(),
            });
        }

        if mode == SynthesisMode::Synthesize {
            for fact in &llm_prop.facts {
                let evidence = resolve_evidence(&fact.evidence, &chunk_by_label);
                match fact.op.as_str() {
                    "add" => {
                        let Some(body) = fact.body.as_ref().filter(|b| !b.trim().is_empty()) else {
                            continue;
                        };
                        items.push(NewProposalItem {
                            id: generate_id("item_"),
                            item_type: "fact_add".into(),
                            target_id: None,
                            payload: serde_json::json!({
                                "body": body,
                                "tags": fact.tags,
                                "confidence": fact.confidence.as_deref().unwrap_or("inferred"),
                            }),
                            evidence,
                        });
                    }
                    "update" => {
                        let Some(target_id) = fact.target_id.as_ref() else {
                            continue;
                        };
                        if !injected_fact_ids.contains(target_id.as_str()) {
                            continue;
                        }
                        let Some(body) = fact.body.as_ref().filter(|b| !b.trim().is_empty()) else {
                            continue;
                        };
                        items.push(NewProposalItem {
                            id: generate_id("item_"),
                            item_type: "fact_update".into(),
                            target_id: Some(target_id.clone()),
                            payload: serde_json::json!({
                                "body": body,
                                "tags": fact.tags,
                                "confidence": fact.confidence.as_deref().unwrap_or("inferred"),
                            }),
                            evidence,
                        });
                    }
                    "archive" => {
                        let Some(target_id) = fact.target_id.as_ref() else {
                            continue;
                        };
                        if !injected_fact_ids.contains(target_id.as_str()) {
                            continue;
                        }
                        items.push(NewProposalItem {
                            id: generate_id("item_"),
                            item_type: "fact_archive".into(),
                            target_id: Some(target_id.clone()),
                            payload: serde_json::json!({}),
                            evidence,
                        });
                    }
                    _ => {}
                }
            }

            for edge in &llm_prop.edges {
                items.push(NewProposalItem {
                    id: generate_id("item_"),
                    item_type: "edge_add".into(),
                    target_id: None,
                    payload: serde_json::json!({
                        "source": edge.source,
                        "target": edge.target,
                        "edge_type": edge.edge_type,
                    }),
                    evidence: Vec::new(),
                });
            }

            for task in &llm_prop.tasks {
                if task.description.trim().is_empty() {
                    continue;
                }
                items.push(NewProposalItem {
                    id: generate_id("item_"),
                    item_type: "task_add".into(),
                    target_id: None,
                    payload: serde_json::json!({
                        "description": task.description,
                        "priority": 0,
                    }),
                    evidence: resolve_evidence(&task.evidence, &chunk_by_label),
                });
            }
        }

        if items.is_empty() {
            continue;
        }

        let proposal_id = generate_id("prop_");
        let sources = collect_sources(trigger_doc_id, &items, &chunk_doc_ids);
        validated.push(ValidatedProposal {
            proposal: NewProposal {
                id: proposal_id,
                kind,
                entity_id,
                proposed_name,
                proposed_type,
                reasoning: Some(llm_prop.reasoning.clone()),
                model: model.to_string(),
            },
            items,
            sources,
        });
    }
    Ok(validated)
}

fn write_synthesis_error(vault_root: Option<&Path>, msg: &str) {
    let Some(vault) = vault_root else {
        return;
    };
    let brain_dir = vault.join(".brain");
    let _ = std::fs::create_dir_all(&brain_dir);
    let log_path = brain_dir.join("errors.log");
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{timestamp}] {msg}\n");
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

fn write_synthesized_event(conn: &Connection, entity_id: &str, summary: &str) -> Result<()> {
    let event_id = generate_id("evt_");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, related_entry_id, created_at)
         VALUES (?1, ?2, 'synthesized', ?3, NULL, ?4)",
        params![event_id, entity_id, summary, now_ms],
    )?;
    Ok(())
}

fn parse_and_validate(
    raw_json: &str,
    mode: SynthesisMode,
    candidates: &[CandidateEntity],
    numbered: &[NumberedChunk],
    model: &str,
    trigger_doc_id: i64,
) -> Result<Vec<ValidatedProposal>> {
    let stripped = strip_json_fences(raw_json);
    let parsed: LlmSynthesisOutput =
        serde_json::from_str(&stripped).context("invalid synthesis JSON")?;
    validate_llm_output(&parsed, mode, candidates, numbered, model, trigger_doc_id)
}

fn call_llm_with_retry(completer: &dyn LlmCompleter, system: &str, user: &str) -> Result<String> {
    let first = completer.complete(system, user)?;
    match parse_json_only(&first) {
        Ok(_) => return Ok(first),
        Err(first_err) => {
            let retry_user = format!(
                "{user}\n\nYour previous response was invalid JSON: {first_err}. \
                 Reply with ONLY valid JSON matching the schema."
            );
            let second = completer.complete(system, &retry_user)?;
            parse_json_only(&second)?;
            Ok(second)
        }
    }
}

fn parse_json_only(raw: &str) -> Result<()> {
    let stripped = strip_json_fences(raw);
    let _: LlmSynthesisOutput = serde_json::from_str(&stripped)?;
    Ok(())
}

fn auto_approve_proposal(conn: &mut Connection, proposal_id: &str) -> Result<()> {
    let detail =
        get_proposal_detail(conn, proposal_id)?.context("proposal missing after insert")?;
    let decisions: Vec<ItemDecision> = detail
        .items
        .iter()
        .map(|item| ItemDecision {
            item_id: item.id.clone(),
            decision: ItemDecisionKind::Accept,
            edited_payload: None,
        })
        .collect();
    resolve_proposal(
        conn,
        proposal_id,
        &decisions,
        None,
        ResolveOptions { auto_approve: true },
    )?;
    Ok(())
}

pub fn run_synthesis(
    conn: &mut Connection,
    source_path: &str,
    source_chunks: &[ChunkRow],
    trigger_doc_id: i64,
    model: &str,
    mode: SynthesisMode,
    auto_approve: bool,
    vault_root: Option<&Path>,
) -> Result<()> {
    let completer = build_llm_completer(model)?.context("LLM provider not configured")?;
    run_synthesis_with_completer(
        conn,
        source_path,
        source_chunks,
        trigger_doc_id,
        model,
        mode,
        auto_approve,
        vault_root,
        completer.as_ref(),
    )
}

pub(crate) fn run_synthesis_with_completer(
    conn: &mut Connection,
    source_path: &str,
    source_chunks: &[ChunkRow],
    trigger_doc_id: i64,
    model: &str,
    mode: SynthesisMode,
    auto_approve: bool,
    vault_root: Option<&Path>,
    completer: &dyn LlmCompleter,
) -> Result<()> {
    let chunk_ids: Vec<i64> = source_chunks.iter().map(|c| c.id).collect();
    let mean_embedding = mean_chunk_embedding(conn, &chunk_ids)?;
    let candidates =
        load_candidate_entities(conn, source_path, source_chunks, mean_embedding.as_deref())?;
    let numbered = load_numbered_chunks(conn, source_chunks)?;

    let context_body = assemble_numbered_context(source_chunks, &numbered);
    let structural = build_structural_context(conn, source_chunks);
    let candidates_section = format_candidates_section(&candidates);
    let full_context = if structural.is_empty() {
        format!("{candidates_section}{context_body}")
    } else {
        format!("{candidates_section}{context_body}\n{structural}")
    };
    let truncated = truncate_context(&full_context);

    let system = build_system_prompt(mode);
    let user = format!("Document to analyze:\n\n{truncated}");

    let raw = match call_llm_with_retry(completer, system, &user) {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("synthesis JSON failure for {source_path}: {e:#}");
            write_synthesis_error(vault_root, &msg);
            return Ok(());
        }
    };

    let validated =
        match parse_and_validate(&raw, mode, &candidates, &numbered, model, trigger_doc_id) {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("synthesis validation failure for {source_path}: {e:#}");
                write_synthesis_error(vault_root, &msg);
                return Ok(());
            }
        };

    if validated.is_empty() {
        return Ok(());
    }

    for bundle in validated {
        let proposal_id = bundle.proposal.id.clone();
        insert_proposal(conn, &bundle.proposal, &bundle.items, &bundle.sources)?;

        if auto_approve {
            auto_approve_proposal(conn, &proposal_id)?;
        } else if let Some(entity_id) = bundle.proposal.entity_id.as_deref() {
            let label = bundle
                .proposal
                .proposed_name
                .as_deref()
                .unwrap_or(entity_id);
            write_synthesized_event(
                conn,
                entity_id,
                &format!("Synthesized proposal for *{label}*"),
            )?;
        } else if let Some(name) = bundle.proposal.proposed_name.as_deref() {
            // new_entity — entity_id assigned at commit; event written by resolve_proposal on auto_approve
            let _ = name;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::{Chunk, ChunkStrategyTag};
    use crate::db::connection::open_in_memory;
    use crate::db::proposals::{list_proposals, ProposalFilter};
    use crate::db::queries::{insert_chunk, upsert_document};

    struct MockCompleter {
        responses: Vec<String>,
        call: std::sync::atomic::AtomicUsize,
    }

    impl MockCompleter {
        fn new(responses: Vec<String>) -> Self {
            Self {
                responses,
                call: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    impl LlmCompleter for MockCompleter {
        fn complete(&self, _system: &str, _user: &str) -> Result<String> {
            let idx = self.call.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.responses
                .get(idx)
                .cloned()
                .context("no more mock responses")
        }
    }

    fn seed_doc_and_chunk(conn: &Connection, path: &str, text: &str) -> (i64, i64) {
        let doc_id = upsert_document(conn, path, "hash").unwrap();
        let chunk = Chunk {
            text: text.into(),
            start_line: 1,
            end_line: 3,
            symbol_name: None,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Prose,
        };
        let chunk_id = insert_chunk(conn, doc_id, &chunk, 0, "tier_fact", "").unwrap();
        // Backfill the post-migration content_hash so resolve_evidence
        // can persist a real hash onto proposal evidence.
        let hash = crate::db::chunk_hash::compute_chunk_hash(text, path, 0);
        conn.execute(
            "UPDATE chunks SET content_hash = ?1 WHERE id = ?2",
            params![hash, chunk_id],
        )
        .unwrap();
        (doc_id, chunk_id)
    }

    fn seed_entity_with_fact(conn: &Connection, entity_id: &str, name: &str, fact_id: &str) {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES (?1, ?2, 'concept', 'Summary text', 100, 100)",
            params![entity_id, name],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence, source_type, created_at, updated_at)
             VALUES (?1, ?2, 'Fact', 'Existing fact body', '[]', 'inferred', 'user_confirmed', 100, 100)",
            params![fact_id, entity_id],
        )
        .unwrap();
    }

    #[test]
    fn strip_json_fences_removes_wrapper() {
        let raw = "```json\n{\"proposals\":[]}\n```";
        assert_eq!(strip_json_fences(raw), "{\"proposals\":[]}");
    }

    #[test]
    fn validate_drops_hallucinated_target_id() {
        let numbered = vec![NumberedChunk {
            label: "C1".into(),
            chunk_id: 1,
            doc_id: 10,
            text: "quote".into(),
            start_line: 1,
            end_line: 2,
            content_hash: "a".repeat(32),
        }];
        let candidates = vec![CandidateEntity {
            id: "ent-1".into(),
            name: "Alpha".into(),
            entity_type: "concept".into(),
            summary_snippet: "Sum".into(),
            facts: vec![CandidateFact {
                id: "fact-real".into(),
                body: "body".into(),
            }],
        }];
        let raw = LlmSynthesisOutput {
            proposals: vec![LlmProposal {
                target: LlmTarget::Existing {
                    existing_id: "ent-1".into(),
                },
                reasoning: "Because".into(),
                summary_update: None,
                facts: vec![
                    LlmFact {
                        op: "update".into(),
                        target_id: Some("fact-hallucinated".into()),
                        body: Some("Nope".into()),
                        tags: vec![],
                        confidence: None,
                        evidence: vec!["C1".into()],
                    },
                    LlmFact {
                        op: "add".into(),
                        target_id: None,
                        body: Some("Valid add".into()),
                        tags: vec![],
                        confidence: None,
                        evidence: vec!["C1".into()],
                    },
                ],
                edges: vec![],
                tasks: vec![],
            }],
        };
        let validated = validate_llm_output(
            &raw,
            SynthesisMode::Synthesize,
            &candidates,
            &numbered,
            "test",
            10,
        )
        .unwrap();
        assert_eq!(validated.len(), 1);
        assert_eq!(validated[0].items.len(), 1);
        assert_eq!(validated[0].items[0].item_type, "fact_add");
    }

    #[test]
    fn validate_drops_unknown_chunk_refs() {
        let numbered = vec![NumberedChunk {
            label: "C1".into(),
            chunk_id: 1,
            doc_id: 10,
            text: "quote".into(),
            start_line: 1,
            end_line: 2,
            content_hash: "a".repeat(32),
        }];
        let raw = LlmSynthesisOutput {
            proposals: vec![LlmProposal {
                target: LlmTarget::New {
                    new: LlmNewEntity {
                        name: "Beta".into(),
                        entity_type: "concept".into(),
                    },
                },
                reasoning: "New entity".into(),
                summary_update: None,
                facts: vec![LlmFact {
                    op: "add".into(),
                    target_id: None,
                    body: Some("Fact".into()),
                    tags: vec![],
                    confidence: None,
                    evidence: vec!["C99".into(), "C1".into()],
                }],
                edges: vec![],
                tasks: vec![],
            }],
        };
        let validated =
            validate_llm_output(&raw, SynthesisMode::Synthesize, &[], &numbered, "test", 10)
                .unwrap();
        assert_eq!(validated[0].items[0].evidence.len(), 1);
        assert_eq!(validated[0].items[0].evidence[0].chunk_id, Some(1));
    }

    #[test]
    fn summarize_mode_ignores_facts_edges_tasks() {
        let numbered = vec![NumberedChunk {
            label: "C1".into(),
            chunk_id: 1,
            doc_id: 10,
            text: "quote".into(),
            start_line: 1,
            end_line: 2,
            content_hash: "a".repeat(32),
        }];
        let raw = LlmSynthesisOutput {
            proposals: vec![LlmProposal {
                target: LlmTarget::New {
                    new: LlmNewEntity {
                        name: "Gamma".into(),
                        entity_type: "project".into(),
                    },
                },
                reasoning: "Summary only".into(),
                summary_update: Some("New summary prose.".into()),
                facts: vec![LlmFact {
                    op: "add".into(),
                    target_id: None,
                    body: Some("Should be dropped".into()),
                    tags: vec![],
                    confidence: None,
                    evidence: vec!["C1".into()],
                }],
                edges: vec![LlmEdge {
                    source: serde_json::json!("self"),
                    target: serde_json::json!("self"),
                    edge_type: "related".into(),
                }],
                tasks: vec![LlmTask {
                    description: "task".into(),
                    evidence: vec![],
                }],
            }],
        };
        let validated =
            validate_llm_output(&raw, SynthesisMode::Summarize, &[], &numbered, "test", 10)
                .unwrap();
        assert_eq!(validated.len(), 1);
        assert_eq!(validated[0].items.len(), 1);
        assert_eq!(validated[0].items[0].item_type, "summary_update");
    }

    #[test]
    fn malformed_json_retries_then_writes_no_proposal() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_doc_and_chunk(
            &conn,
            "/vault/documents/note.md",
            "content about Alpha project",
        );
        let chunks = vec![ChunkRow {
            id: chunk_id,
            entity_id: "tier_working::abc".into(),
            text: "content about Alpha project".into(),
            symbol_name: None,
            start_line: 1,
            end_line: 3,
            tier: "user_doc".into(),
            path: "/vault/documents/note.md".into(),
        }];

        let mock = MockCompleter::new(vec!["not json".into(), "still not json".into()]);
        let tmp = tempfile::tempdir().unwrap();
        let result = run_synthesis_with_completer(
            &mut conn,
            "/vault/documents/note.md",
            &chunks,
            doc_id,
            "test-model",
            SynthesisMode::Synthesize,
            false,
            Some(tmp.path()),
            &mock,
        );
        assert!(result.is_ok());
        let queue = list_proposals(&conn, &ProposalFilter::default()).unwrap();
        assert!(queue.is_empty());

        let log = std::fs::read_to_string(tmp.path().join(".brain").join("errors.log")).unwrap();
        assert!(log.contains("synthesis JSON failure"));
    }

    #[test]
    fn valid_synthesis_inserts_proposal() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) = seed_doc_and_chunk(
            &conn,
            "/vault/documents/note.md",
            "Alpha project details here",
        );
        seed_entity_with_fact(&conn, "ent-alpha", "Alpha", "fact-1");

        let json = serde_json::json!({
            "proposals": [{
                "target": { "existing_id": "ent-alpha" },
                "reasoning": "Doc adds a fact.",
                "summary_update": null,
                "facts": [{
                    "op": "add",
                    "target_id": null,
                    "body": "Alpha uses Rust.",
                    "tags": [],
                    "confidence": "inferred",
                    "evidence": ["C1"]
                }],
                "edges": [],
                "tasks": []
            }]
        })
        .to_string();

        let mock = MockCompleter::new(vec![json]);
        let chunks = vec![ChunkRow {
            id: chunk_id,
            entity_id: "tier_working::abc".into(),
            text: "Alpha project details here".into(),
            symbol_name: None,
            start_line: 1,
            end_line: 3,
            tier: "user_doc".into(),
            path: "/vault/documents/note.md".into(),
        }];

        run_synthesis_with_completer(
            &mut conn,
            "/vault/documents/note.md",
            &chunks,
            doc_id,
            "test-model",
            SynthesisMode::Synthesize,
            false,
            None,
            &mock,
        )
        .unwrap();

        let queue = list_proposals(&conn, &ProposalFilter::default()).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].target_name, "Alpha");
        assert_eq!(queue[0].item_counts.facts, 1);

        // Phase 9: persisted evidence must carry the source chunk's
        // `content_hash` so a later chunk replacement (which re-issues
        // the rowid) cannot orphan the proposal's anchor.
        let evidence_json: String = conn
            .query_row(
                "SELECT evidence FROM curated_proposal_items
                 WHERE proposal_id = ?1",
                [queue[0].id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        let evidence: serde_json::Value = serde_json::from_str(&evidence_json).unwrap();
        let entry = evidence.as_array().unwrap()[0].as_object().unwrap();
        let hash = entry.get("content_hash").and_then(|v| v.as_str()).unwrap();
        assert_eq!(
            hash.len(),
            32,
            "persisted evidence content_hash must be the 32-char hex"
        );
        assert_ne!(
            hash, "",
            "persisted evidence content_hash must not be empty"
        );
    }

    #[test]
    fn auto_approve_commits_proposal() {
        let mut conn = open_in_memory().unwrap();
        let (doc_id, chunk_id) =
            seed_doc_and_chunk(&conn, "/vault/documents/auto.md", "Beta system overview");
        seed_entity_with_fact(&conn, "ent-beta", "Beta", "fact-b");

        let json = serde_json::json!({
            "proposals": [{
                "target": { "existing_id": "ent-beta" },
                "reasoning": "Auto path.",
                "summary_update": null,
                "facts": [{
                    "op": "add",
                    "body": "Beta is deployed.",
                    "tags": [],
                    "confidence": "inferred",
                    "evidence": ["C1"]
                }],
                "edges": [],
                "tasks": []
            }]
        })
        .to_string();

        let mock = MockCompleter::new(vec![json]);
        let chunks = vec![ChunkRow {
            id: chunk_id,
            entity_id: "tier_working::abc".into(),
            text: "Beta system overview".into(),
            symbol_name: None,
            start_line: 1,
            end_line: 3,
            tier: "user_doc".into(),
            path: "/vault/documents/auto.md".into(),
        }];

        run_synthesis_with_completer(
            &mut conn,
            "/vault/documents/auto.md",
            &chunks,
            doc_id,
            "test-model",
            SynthesisMode::Synthesize,
            true,
            None,
            &mock,
        )
        .unwrap();

        let pending = list_proposals(&conn, &ProposalFilter::default()).unwrap();
        assert!(pending.is_empty());

        let fact_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE source_type = 'librarian_inferred'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fact_count, 1);
    }
}
