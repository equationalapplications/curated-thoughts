//! Extension-based chunk strategy classification (deterministic, no I/O).

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AstLang {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkStrategy {
    AstSymbol(AstLang),
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
        "md" | "markdown"
            | "txt"
            | "rst"
            | "org"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "rs"
            | "py"
            | "go"
            | "java"
            | "kt"
            | "swift"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "cs"
            | "rb"
            | "php"
            | "vue"
            | "svelte"
            | "yaml"
            | "yml"
            | "json"
            | "jsonc"
            | "toml"
            | "xml"
            | "pdf"
            | "docx"
    )
}

pub fn classify(path: &Path) -> ChunkStrategy {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return ChunkStrategy::Fallback;
    };
    let ext = ext.to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" | "txt" | "rst" | "org" => ChunkStrategy::Prose,
        "rs" => ChunkStrategy::AstSymbol(AstLang::Rust),
        "ts" | "tsx" => ChunkStrategy::AstSymbol(AstLang::TypeScript),
        "js" | "jsx" | "mjs" | "cjs" => ChunkStrategy::AstSymbol(AstLang::JavaScript),
        "py" => ChunkStrategy::AstSymbol(AstLang::Python),
        "go" => ChunkStrategy::AstSymbol(AstLang::Go),
        "java" | "kt" | "swift" | "c" | "h" | "cpp" | "hpp" | "cs" | "rb" | "php" | "vue"
        | "svelte" => ChunkStrategy::CodeLike,
        "yaml" | "yml" | "json" | "jsonc" | "toml" | "xml" => ChunkStrategy::Declarative,
        _ => ChunkStrategy::Fallback,
    }
}

/// True when we must parse with the TSX grammar (`.tsx` sources).
#[inline]
pub fn path_uses_tsx(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("tsx"))
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
    fn ast_symbol_extensions() {
        assert_eq!(
            classify(&p("main.rs")),
            ChunkStrategy::AstSymbol(AstLang::Rust)
        );
        assert_eq!(
            classify(&p("app.ts")),
            ChunkStrategy::AstSymbol(AstLang::TypeScript)
        );
        assert_eq!(
            classify(&p("ui.tsx")),
            ChunkStrategy::AstSymbol(AstLang::TypeScript)
        );
        assert_eq!(
            classify(&p("index.js")),
            ChunkStrategy::AstSymbol(AstLang::JavaScript)
        );
        assert_eq!(
            classify(&p("comp.jsx")),
            ChunkStrategy::AstSymbol(AstLang::JavaScript)
        );
        assert_eq!(
            classify(&p("util.mjs")),
            ChunkStrategy::AstSymbol(AstLang::JavaScript)
        );
        assert_eq!(
            classify(&p("mod.cjs")),
            ChunkStrategy::AstSymbol(AstLang::JavaScript)
        );
        assert_eq!(
            classify(&p("script.py")),
            ChunkStrategy::AstSymbol(AstLang::Python)
        );
        assert_eq!(
            classify(&p("service.go")),
            ChunkStrategy::AstSymbol(AstLang::Go)
        );
    }

    #[test]
    fn scanner_extensions() {
        assert_eq!(classify(&p("Main.java")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("App.kt")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("View.swift")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("main.c")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("main.cpp")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("page.vue")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("comp.svelte")), ChunkStrategy::CodeLike);
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
        assert_eq!(
            classify(Path::new("/no/extension")),
            ChunkStrategy::Fallback
        );
        assert_eq!(classify(&p("doc.pdf")), ChunkStrategy::Fallback);
        assert_eq!(classify(&p("paper.docx")), ChunkStrategy::Fallback);
    }
}
