//! Typed model → bundle files (§9 producer checklist).

use std::collections::HashMap;

use crate::okf::entity_index_md::build_entity_index_md;
use crate::okf::event_line::build_event_text;
use crate::okf::fact_file::build_fact_file;
use crate::okf::index_md::build_root_index_md;
use crate::okf::log_md::build_log_md;
use crate::okf::sanitize::{sanitize_concept_id, sanitize_for_filename};
use crate::okf::task_file::build_task_file;
use crate::okf::types::{
    OkfFile, OkfIndexEntry, OkfIndexSection, OkfLogEntry, WikiFact, WikiTask, LLM_WIKI_PROFILE,
};

#[derive(Debug, Clone)]
pub struct ExportEvent {
    /// Empty string means "no stable id" (never expected from our DB export;
    /// tolerated so parse→write round trips profile-0 data).
    pub event_id: String,
    pub event_type: String,
    pub summary: String,
    pub related_entry_id: Option<String>,
    /// UTC `YYYY-MM-DD`.
    pub date: String,
}

#[derive(Debug, Clone)]
pub struct ExportEntity {
    pub entity_id: String,
    pub display_name: String,
    pub summary: Option<String>,
    pub facts: Vec<WikiFact>,
    pub tasks: Vec<WikiTask>,
    /// (source_id, target_id, edge_type)
    pub edges: Vec<(String, String, String)>,
    pub events: Vec<ExportEvent>,
}

enum ConceptKind {
    Fact,
    Task,
}

pub fn write_bundle(entities: &[ExportEntity]) -> Vec<OkfFile> {
    let mut files = Vec::new();
    let mut root_entries = Vec::new();

    for entity in entities {
        let dir = sanitize_for_filename(&entity.entity_id);
        let base = format!("entities/{dir}");

        let mut concepts: HashMap<&str, (ConceptKind, String)> = HashMap::new();
        for fact in &entity.facts {
            concepts.insert(
                fact.id.as_str(),
                (ConceptKind::Fact, format!("{}.md", sanitize_concept_id(&fact.id))),
            );
        }
        for task in &entity.tasks {
            concepts.insert(
                task.id.as_str(),
                (ConceptKind::Task, format!("{}.md", sanitize_concept_id(&task.id))),
            );
        }

        let related_for = |source_id: &str, source_kind: &ConceptKind| -> Vec<(String, String)> {
            entity
                .edges
                .iter()
                .filter(|(s, _, _)| s == source_id)
                .filter_map(|(_, target, edge_type)| {
                    let (target_kind, target_file) = concepts.get(target.as_str())?;
                    let path = match (source_kind, target_kind) {
                        (ConceptKind::Fact, ConceptKind::Fact)
                        | (ConceptKind::Task, ConceptKind::Task) => format!("./{target_file}"),
                        (ConceptKind::Fact, ConceptKind::Task) => format!("../tasks/{target_file}"),
                        (ConceptKind::Task, ConceptKind::Fact) => format!("../facts/{target_file}"),
                    };
                    Some((edge_type.clone(), path))
                })
                .collect()
        };

        let mut fact_entries = Vec::new();
        for fact in &entity.facts {
            let file_name = &concepts[fact.id.as_str()].1;
            files.push(OkfFile {
                path: format!("{base}/facts/{file_name}"),
                content: build_fact_file(fact, &related_for(&fact.id, &ConceptKind::Fact)),
            });
            fact_entries.push(OkfIndexEntry {
                title: fact.title.clone(),
                path: format!("facts/{file_name}"),
                description: None,
            });
        }

        let mut task_entries = Vec::new();
        for task in &entity.tasks {
            let file_name = &concepts[task.id.as_str()].1;
            files.push(OkfFile {
                path: format!("{base}/tasks/{file_name}"),
                content: build_task_file(task, &related_for(&task.id, &ConceptKind::Task)),
            });
            task_entries.push(OkfIndexEntry {
                title: task.description.clone(),
                path: format!("tasks/{file_name}"),
                description: None,
            });
        }

        let mut sections = Vec::new();
        if !fact_entries.is_empty() {
            sections.push(OkfIndexSection {
                heading: "Facts".into(),
                entries: fact_entries,
            });
        }
        if !task_entries.is_empty() {
            sections.push(OkfIndexSection {
                heading: "Tasks".into(),
                entries: task_entries,
            });
        }
        files.push(OkfFile {
            path: format!("{base}/index.md"),
            content: build_entity_index_md(entity.summary.as_deref(), &sections),
        });

        let log_entries: Vec<OkfLogEntry> = entity
            .events
            .iter()
            .map(|ev| {
                let related_path = ev
                    .related_entry_id
                    .as_deref()
                    .and_then(|id| concepts.get(id))
                    .map(|(kind, file)| match kind {
                        ConceptKind::Fact => format!("./facts/{file}"),
                        ConceptKind::Task => format!("./tasks/{file}"),
                    });
                OkfLogEntry {
                    date: ev.date.clone(),
                    text: build_event_text(
                        &ev.event_type,
                        &ev.summary,
                        related_path.as_deref(),
                        &ev.event_id,
                    ),
                }
            })
            .collect();
        files.push(OkfFile {
            path: format!("{base}/log.md"),
            content: build_log_md(&log_entries),
        });

        root_entries.push(OkfIndexEntry {
            title: entity.display_name.clone(),
            path: format!("entities/{dir}/index.md"),
            description: None,
        });
    }

    files.push(OkfFile {
        path: "index.md".into(),
        content: build_root_index_md(
            "0.1",
            &[OkfIndexSection {
                heading: "Entities".into(),
                entries: root_entries,
            }],
            Some(LLM_WIKI_PROFILE),
        ),
    });

    files
}
