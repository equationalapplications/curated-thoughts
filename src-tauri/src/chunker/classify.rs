//! Extension-based chunk strategy classification (deterministic, no I/O).

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkStrategy {
    Prose,
    CodeLike,
    Declarative,
    Fallback,
}

/// Extensions the pipeline indexes (binary formats use extractors; others UTF-8 / lossy).
pub fn should_ingest_extension(raw_ext: &str) -> bool {
    let ext = raw_ext.to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "md" | "markdown" | "txt" | "rst" | "org" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs"
            | "rs" | "py" | "go" | "java" | "kt" | "swift" | "c" | "h" | "cpp" | "hpp" | "cs"
            | "rb" | "php" | "vue" | "svelte" | "yaml" | "yml" | "json" | "jsonc" | "toml"
            | "xml" | "pdf" | "docx"
    )
}

pub fn classify(path: &Path) -> ChunkStrategy {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return ChunkStrategy::Fallback;
    };
    let ext = ext.to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" | "txt" | "rst" | "org" => ChunkStrategy::Prose,
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "rs" | "py" | "go" | "java"
        | "kt" | "swift" | "c" | "h" | "cpp" | "hpp" | "cs" | "rb" | "php" | "vue"
        | "svelte" => ChunkStrategy::CodeLike,
        "yaml" | "yml" | "json" | "jsonc" | "toml" | "xml" => ChunkStrategy::Declarative,
        _ => ChunkStrategy::Fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(name: &str) -> PathBuf {
        PathBuf::from("/vault/documents").join(name)
    }

    #[test]
    fn prose_extensions() {
        assert_eq!(classify(&p("note.md")), ChunkStrategy::Prose);
        assert_eq!(classify(&p("README.markdown")), ChunkStrategy::Prose);
        assert_eq!(classify(&p("LICENSE.txt")), ChunkStrategy::Prose);
        assert_eq!(classify(&p("guide.rst")), ChunkStrategy::Prose);
        assert_eq!(classify(&p("tasks.org")), ChunkStrategy::Prose);
    }

    #[test]
    fn code_extensions() {
        assert_eq!(classify(&p("app.ts")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("ui.tsx")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("mod.rs")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("foo.jsx")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("page.vue")), ChunkStrategy::CodeLike);
    }

    #[test]
    fn declarative_extensions() {
        assert_eq!(classify(&p("cfg.yaml")), ChunkStrategy::Declarative);
        assert_eq!(classify(&p("cfg.yml")), ChunkStrategy::Declarative);
        assert_eq!(classify(&p("data.json")), ChunkStrategy::Declarative);
        assert_eq!(classify(&p("tsconf.jsonc")), ChunkStrategy::Declarative);
        assert_eq!(classify(&p("Cargo.toml")), ChunkStrategy::Declarative);
        assert_eq!(classify(&p("layout.xml")), ChunkStrategy::Declarative);
    }

    #[test]
    fn fallback_unknown_or_missing_ext() {
        assert_eq!(classify(&p("Makefile")), ChunkStrategy::Fallback);
        assert_eq!(classify(&p("Dockerfile")), ChunkStrategy::Fallback);
        assert_eq!(classify(&p("bin/tool")), ChunkStrategy::Fallback);
        assert_eq!(classify(Path::new("/no/extension")), ChunkStrategy::Fallback);
        assert_eq!(classify(&p("doc.pdf")), ChunkStrategy::Fallback);
        assert_eq!(classify(&p("paper.docx")), ChunkStrategy::Fallback);
    }
}
