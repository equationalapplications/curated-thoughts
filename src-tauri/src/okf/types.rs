use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const LLM_WIKI_PROFILE: &str = "llm-wiki/1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OkfFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct OkfFrontmatter {
    pub fields: HashMap<String, OkfFrontmatterValue>,
}

impl OkfFrontmatter {
    pub fn get_str(&self, key: &str) -> Option<&str> {
        match self.fields.get(key)? {
            OkfFrontmatterValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn get_number(&self, key: &str) -> Option<f64> {
        match self.fields.get(key)? {
            OkfFrontmatterValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn get_string_list(&self, key: &str) -> Option<&[String]> {
        match self.fields.get(key)? {
            OkfFrontmatterValue::StringList(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    pub fn insert_str(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.fields
            .insert(key.into(), OkfFrontmatterValue::String(value.into()));
    }

    pub fn insert_number(&mut self, key: impl Into<String>, value: f64) {
        self.fields
            .insert(key.into(), OkfFrontmatterValue::Number(value));
    }

    pub fn insert_null(&mut self, key: impl Into<String>) {
        self.fields.insert(key.into(), OkfFrontmatterValue::Null);
    }

    pub fn insert_string_list(&mut self, key: impl Into<String>, values: Vec<String>) {
        self.fields.insert(
            key.into(),
            OkfFrontmatterValue::StringList(values),
        );
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum OkfFrontmatterValue {
    String(String),
    Number(f64),
    Bool(bool),
    Null,
    StringList(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OkfIndexEntry {
    pub path: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OkfIndexSection {
    pub heading: String,
    pub entries: Vec<OkfIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OkfLogEntry {
    pub date: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OkfMarkdownLink {
    pub text: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiFact {
    pub id: String,
    pub entity_id: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub confidence: String,
    pub source_type: String,
    pub source_hash: Option<String>,
    pub source_ref: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_accessed_at: Option<i64>,
    pub access_count: i64,
    pub deleted_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub okf_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiTask {
    pub id: String,
    pub entity_id: String,
    pub description: String,
    pub status: String,
    pub priority: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub resolved_at: Option<i64>,
    pub deleted_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub okf_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiEvent {
    pub id: String,
    pub entity_id: String,
    pub event_type: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_entry_id: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiEdge {
    pub id: String,
    pub entity_id: String,
    pub source_id: String,
    pub target_id: String,
    pub edge_type: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct EntityBundle {
    pub facts: Vec<WikiFact>,
    pub tasks: Vec<WikiTask>,
    pub events: Vec<WikiEvent>,
    #[serde(default)]
    pub edges: Vec<WikiEdge>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryDump {
    pub generated_at: i64,
    pub entities: HashMap<String, EntityBundle>,
}

#[derive(Debug, Clone, Default)]
pub struct OkfImportOptions {
    pub type_mapping: HashMap<String, OkfRoute>,
    pub default_schema: Option<OkfRoute>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OkfRoute {
    Fact,
    Task,
    Ignore,
}
