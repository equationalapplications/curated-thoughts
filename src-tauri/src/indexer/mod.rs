//! Pass 2 reference extraction (call sites, import statements).
//! Pass 1 (symbol definitions) is handled by `chunker::ast_symbol`.
//! Pass 3 (global resolver / linker) is in `indexer::linker`.

pub mod linker;

use crate::chunker::{Chunk, ChunkStrategyTag};
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

/// Extract reference chunks from source text for a given language.
/// Returns chunks with `symbol_name` set to the called/imported symbol (lowercase),
/// and `defined_symbol = None`.
pub fn extract_references(lang: RefLang, text: &str, base_line: u32) -> Vec<Chunk> {
    let (ts_lang, pattern) = lang_query(lang);
    let mut parser = Parser::new();
    if parser.set_language(&ts_lang).is_err() {
        return vec![];
    }
    let Some(tree) = parser.parse(text, None) else {
        return vec![];
    };
    let Ok(query) = Query::new(&ts_lang, pattern) else {
        return vec![];
    };
    let mut cursor = QueryCursor::new();
    let source = text.as_bytes();
    let ref_idx = query.capture_index_for_name("ref.name");
    let Some(ref_idx) = ref_idx else {
        return vec![];
    };

    let mut chunks = Vec::new();
    let mut mq = cursor.matches(&query, tree.root_node(), source);
    loop {
        mq.advance();
        let Some(mat) = mq.get() else { break };
        for cap in mat.captures {
            if cap.index != ref_idx {
                continue;
            }
            let node = cap.node;
            let Ok(name) = node.utf8_text(source) else { continue };
            let name = name.trim().to_lowercase();
            if name.is_empty() {
                continue;
            }
            let context_node = node.parent()
                .and_then(|p| {
                    // For nested captures (e.g. field_identifier inside field_expression inside
                    // call_expression, or identifier inside import_specifier inside import_statement),
                    // walk two levels up to include the full call/import context.
                    p.parent().filter(|gp| {
                        matches!(gp.kind(),
                            "call_expression" | "call" | "import_statement" |
                            "use_declaration" | "import_declaration"
                        )
                    }).or(Some(p))
                })
                .unwrap_or(node);
            let start_byte = context_node.start_byte();
            let end_byte = context_node.end_byte();
            let snippet = text.get(start_byte..end_byte)
                .unwrap_or("")
                .trim()
                .to_string();
            if snippet.is_empty() {
                continue;
            }
            let start_line = base_line
                + text[..start_byte].bytes().filter(|&b| b == b'\n').count() as u32
                + 1;
            let end_line = base_line
                + text[..end_byte].bytes().filter(|&b| b == b'\n').count() as u32
                + 1;
            chunks.push(Chunk {
                text: snippet,
                start_line,
                end_line,
                symbol_name: Some(name),
                defined_symbol: None,
                strategy: ChunkStrategyTag::AstRef,
            });
        }
    }
    deduplicate_by_symbol(chunks)
}

/// Collapse multiple reference chunks with the same symbol into one per file
/// (keeps the first occurrence to avoid flooding the index with repeated call sites).
fn deduplicate_by_symbol(mut chunks: Vec<Chunk>) -> Vec<Chunk> {
    let mut seen = std::collections::HashSet::new();
    chunks.retain(|c| {
        if let Some(sym) = &c.symbol_name {
            seen.insert(sym.clone())
        } else {
            true
        }
    });
    chunks
}

/// Languages supported by the reference extractor.
#[derive(Clone, Copy)]
pub enum RefLang {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
}

fn lang_query(lang: RefLang) -> (Language, &'static str) {
    match lang {
        RefLang::Rust => (
            tree_sitter_rust::LANGUAGE.into(),
            r#"
[
  (call_expression
     function: (identifier) @ref.name)
  (call_expression
     function: (field_expression
       field: (field_identifier) @ref.name))
  (use_declaration
     argument: (scoped_identifier
       name: (identifier) @ref.name))
]
"#,
        ),
        RefLang::TypeScript => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            r#"
[
  (call_expression
     function: (identifier) @ref.name)
  (call_expression
     function: (member_expression
       property: (property_identifier) @ref.name))
  (import_statement
     (import_clause
       (named_imports
         (import_specifier name: (identifier) @ref.name))))
]
"#,
        ),
        RefLang::JavaScript => (
            tree_sitter_javascript::LANGUAGE.into(),
            r#"
[
  (call_expression
     function: (identifier) @ref.name)
  (call_expression
     function: (member_expression
       property: (property_identifier) @ref.name))
]
"#,
        ),
        RefLang::Python => (
            tree_sitter_python::LANGUAGE.into(),
            r#"
[
  (call
     function: (identifier) @ref.name)
  (call
     function: (attribute
       attribute: (identifier) @ref.name))
]
"#,
        ),
        RefLang::Go => (
            tree_sitter_go::LANGUAGE.into(),
            r#"
[
  (call_expression
     function: (identifier) @ref.name)
  (call_expression
     function: (selector_expression
       field: (field_identifier) @ref.name))
]
"#,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_call_expression_extracted() {
        let src = r#"fn main() { init_db(); }"#;
        let refs = extract_references(RefLang::Rust, src, 0);
        assert!(
            refs.iter().any(|c| c.symbol_name.as_deref() == Some("init_db")),
            "expected init_db in refs, got: {:?}",
            refs.iter().map(|c| &c.symbol_name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn reference_chunks_have_none_defined_symbol() {
        let src = r#"fn main() { do_something(); }"#;
        let refs = extract_references(RefLang::Rust, src, 0);
        for r in &refs {
            assert!(r.defined_symbol.is_none(), "defined_symbol should be None for ref chunks");
        }
    }

    #[test]
    fn deduplication_keeps_first_occurrence() {
        let src = r#"fn main() { foo(); foo(); bar(); }"#;
        let refs = extract_references(RefLang::Rust, src, 0);
        let foo_count = refs.iter().filter(|c| c.symbol_name.as_deref() == Some("foo")).count();
        assert_eq!(foo_count, 1, "foo should appear exactly once after dedup");
    }

    #[test]
    fn typescript_import_extracted() {
        let src = r#"import { useState } from 'react';"#;
        let refs = extract_references(RefLang::TypeScript, src, 0);
        assert!(
            refs.iter().any(|c| c.symbol_name.as_deref() == Some("usestate")),
            "expected usestate in refs"
        );
    }

    #[test]
    fn rust_method_call_extracted() {
        let src = r#"fn main() { self.init_db(); }"#;
        let refs = extract_references(RefLang::Rust, src, 0);
        assert!(
            refs.iter().any(|c| c.symbol_name.as_deref() == Some("init_db")),
            "expected init_db in refs from method call, got: {:?}",
            refs.iter().map(|c| &c.symbol_name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn rust_use_declaration_extracted() {
        let src = r#"use crate::db::init_db;"#;
        let refs = extract_references(RefLang::Rust, src, 0);
        assert!(
            refs.iter().any(|c| c.symbol_name.as_deref() == Some("init_db")),
            "expected init_db in refs from use declaration, got: {:?}",
            refs.iter().map(|c| &c.symbol_name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ast_ref_strategy_tag_serializes() {
        assert_eq!(ChunkStrategyTag::AstRef.as_db_str(), "ast_ref");
    }
}
