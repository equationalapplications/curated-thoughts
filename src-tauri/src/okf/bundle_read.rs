//! Bundle → typed model (§8 consumer requirements: path allow-list first,
//! ids from frontmatter never filenames, related stripped, unknown skipped).

use std::collections::HashMap;

use anyhow::Result;

use crate::okf::entity_index_md::parse_entity_index_md;
use crate::okf::event_line::parse_event_text;
use crate::okf::fact_file::parse_fact_file;
use crate::okf::index_md::parse_root_index_md;
use crate::okf::log_md::parse_log_md;
use crate::okf::path_allowlist::is_allowed_okf_path;
use crate::okf::task_file::parse_task_file;
use crate::okf::types::{OkfFile, WikiFact, WikiTask};

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEvent {
    pub event_id: Option<String>,
    pub event_type: String,
    pub summary: String,
    pub related_entry_id: Option<String>,
    pub date: String,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedEntity {
    pub entity_id: String,
    pub display_name: Option<String>,
    pub summary: Option<String>,
    pub facts: Vec<WikiFact>,
    pub tasks: Vec<WikiTask>,
    /// (source_id, target_id, edge_type) — resolved, dangling skipped.
    pub edges: Vec<(String, String, String)>,
    pub events: Vec<ParsedEvent>,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedBundle {
    pub okf_version: Option<String>,
    pub profile: Option<String>,
    pub entities: Vec<ParsedEntity>,
    pub skipped_paths: Vec<String>,
    pub warnings: Vec<String>,
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Defensive percent-decode (§2): tolerate foreign tools that encode paths.
fn percent_decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| path.to_string())
}

fn normalize_path(path: &str) -> String {
    percent_decode(path)
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

/// Resolve a relative link (e.g. `./fact_beta.md`, `../tasks/task_x.md`)
/// against `entities/{dir}/{subdir}/` into a normalized bundle path.
fn resolve_link(entity_dir: &str, subdir: &str, link: &str) -> Option<String> {
    let decoded = normalize_path(link);
    let mut segs: Vec<String> = vec!["entities".into(), entity_dir.into()];
    if !subdir.is_empty() {
        segs.push(subdir.into());
    }
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if segs.len() <= 2 {
                    return None;
                }
                segs.pop();
            }
            other => segs.push(other.into()),
        }
    }
    Some(segs.join("/"))
}

pub fn parse_bundle(files: &[OkfFile]) -> Result<ParsedBundle> {
    let mut bundle = ParsedBundle::default();

    let mut allowed: Vec<(String, &OkfFile)> = Vec::new();
    for file in files {
        let normalized = normalize_path(&file.path);
        if is_allowed_okf_path(&normalized) {
            allowed.push((normalized, file));
        } else {
            bundle.skipped_paths.push(file.path.clone());
        }
    }

    let mut display_names: HashMap<String, String> = HashMap::new();
    if let Some((_, root)) = allowed.iter().find(|(p, _)| p == "index.md") {
        let (version, profile) = parse_root_index_md(&root.content);
        bundle.okf_version = version;
        bundle.profile = profile;
        let (_, sections) = parse_entity_index_md(&root.content);
        for section in sections {
            for entry in section.entries {
                let path = normalize_path(&entry.path);
                if let Some(dir) = path
                    .strip_prefix("entities/")
                    .and_then(|r| r.split('/').next())
                {
                    display_names.insert(dir.to_string(), entry.title);
                }
            }
        }
    }

    let mut by_dir: HashMap<String, Vec<(String, &OkfFile)>> = HashMap::new();
    for (path, file) in &allowed {
        if let Some(rest) = path.strip_prefix("entities/") {
            if let Some((dir, _)) = rest.split_once('/') {
                by_dir
                    .entry(dir.to_string())
                    .or_default()
                    .push((path.clone(), file));
            }
        }
    }

    let mut dirs: Vec<String> = by_dir.keys().cloned().collect();
    dirs.sort();

    for dir in dirs {
        let entity_files = &by_dir[&dir];
        let mut entity = ParsedEntity {
            display_name: display_names.get(&dir).cloned(),
            ..ParsedEntity::default()
        };
        let mut concept_ids: HashMap<String, String> = HashMap::new();
        let mut pending_links: Vec<(String, String, Vec<crate::okf::types::OkfMarkdownLink>)> =
            Vec::new();
        let mut log_content: Option<&str> = None;

        for (path, file) in entity_files {
            let tail = path
                .strip_prefix(&format!("entities/{dir}/"))
                .unwrap_or_default();
            if tail == "index.md" {
                let (summary, _sections) = parse_entity_index_md(&file.content);
                if !summary.is_empty() {
                    entity.summary = Some(summary);
                }
            } else if tail == "log.md" {
                log_content = Some(&file.content);
            } else if tail.starts_with("facts/") {
                match parse_fact_file(&file.content) {
                    Ok(parsed) => {
                        concept_ids.insert(path.clone(), parsed.fact.id.clone());
                        pending_links.push((parsed.fact.id.clone(), "facts".into(), parsed.related));
                        entity.facts.push(parsed.fact);
                    }
                    Err(e) => bundle.warnings.push(format!("skipped {path}: {e}")),
                }
            } else if tail.starts_with("tasks/") {
                match parse_task_file(&file.content) {
                    Ok(parsed) => {
                        concept_ids.insert(path.clone(), parsed.task.id.clone());
                        pending_links.push((parsed.task.id.clone(), "tasks".into(), parsed.related));
                        entity.tasks.push(parsed.task);
                    }
                    Err(e) => bundle.warnings.push(format!("skipped {path}: {e}")),
                }
            }
        }

        entity.entity_id = entity
            .facts
            .first()
            .map(|f| f.entity_id.clone())
            .or_else(|| entity.tasks.first().map(|t| t.entity_id.clone()))
            .unwrap_or_else(|| dir.clone());

        for (source_id, subdir, links) in pending_links {
            for link in links {
                let Some(target_path) = resolve_link(&dir, &subdir, &link.path) else {
                    bundle.warnings.push(format!(
                        "unresolvable link {} from {source_id}",
                        link.path
                    ));
                    continue;
                };
                match concept_ids.get(&target_path) {
                    Some(target_id) => {
                        entity
                            .edges
                            .push((source_id.clone(), target_id.clone(), link.text.clone()));
                    }
                    None => bundle
                        .warnings
                        .push(format!("dangling edge target {target_path} from {source_id}")),
                }
            }
        }

        if let Some(content) = log_content {
            for entry in parse_log_md(content) {
                let Some(parsed) = parse_event_text(&entry.text) else {
                    continue;
                };
                let related_entry_id = parsed.related_path.as_deref().and_then(|p| {
                    resolve_link(&dir, "", p).and_then(|full| concept_ids.get(&full).cloned())
                });
                entity.events.push(ParsedEvent {
                    event_id: parsed.event_id,
                    event_type: parsed.event_type,
                    summary: parsed.summary,
                    related_entry_id,
                    date: entry.date,
                });
            }
        }

        bundle.entities.push(entity);
    }

    Ok(bundle)
}
