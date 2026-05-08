use std::path::PathBuf;

use tauri_app_lib::chunker::{chunk_autodetect, ChunkStrategyTag};

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ast")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("fixture not found: {}", path.display()))
}

#[test]
fn rust_chunk_names_and_strategy() {
    let text = fixture("sample.rs");
    let chunks = chunk_autodetect(&PathBuf::from("sample.rs"), &text);
    let names: Vec<_> = chunks
        .iter()
        .map(|c| c.symbol_name.as_deref().unwrap_or(""))
        .collect();
    assert!(names.contains(&"top_fn"));
    assert!(names.contains(&"Foo"));
    assert!(names.contains(&"Bar"));
    assert!(names.contains(&"Counter"));
    assert!(names.contains(&"Counter::new"));
    assert!(
        chunks
            .iter()
            .all(|c| c.strategy == ChunkStrategyTag::AstSymbolRust),
        "expected AstSymbolRust, strategies: {:?}",
        chunks.iter().map(|c| &c.strategy).collect::<Vec<_>>()
    );
}

#[test]
fn rust_impl_preamble_on_first_method() {
    let text = fixture("sample.rs");
    let chunks = chunk_autodetect(&PathBuf::from("sample.rs"), &text);
    let new_chunk = chunks
        .iter()
        .find(|c| c.symbol_name.as_deref() == Some("Counter::new"))
        .expect("Counter::new chunk");
    assert!(
        new_chunk.text.contains("const MAX"),
        "preamble missing: {}",
        new_chunk.text
    );
}

#[test]
fn python_standalone_methods_and_dataclass() {
    let text = fixture("sample.py");
    let chunks = chunk_autodetect(&PathBuf::from("sample.py"), &text);
    let names: Vec<_> = chunks
        .iter()
        .map(|c| c.symbol_name.as_deref().unwrap_or(""))
        .collect();
    assert!(names.contains(&"standalone"));
    assert!(names.contains(&"Calculator.add"));
    assert!(names.contains(&"Calculator.sub"));
    assert!(names.contains(&"Config"));
    assert!(chunks.iter().all(|c| c.strategy == ChunkStrategyTag::AstSymbolPython));
}

#[test]
fn go_method_names_use_receiver_form() {
    let text = fixture("sample.go");
    let chunks = chunk_autodetect(&PathBuf::from("sample.go"), &text);
    let mut names: Vec<_> = chunks
        .iter()
        .filter_map(|c| c.symbol_name.as_deref())
        .collect();
    names.sort_unstable();
    assert!(names.contains(&"(*Counter).Increment"));
    assert!(names.contains(&"Counter.Value"));
    assert!(chunks.iter().all(|c| c.strategy == ChunkStrategyTag::AstSymbolGo));
}

#[test]
fn ts_export_fn_arrow_interface_type_class_methods() {
    let text = fixture("sample.ts");
    let chunks = chunk_autodetect(&PathBuf::from("sample.ts"), &text);
    let names: Vec<_> = chunks
        .iter()
        .filter_map(|c| c.symbol_name.as_deref())
        .collect();
    assert!(names.contains(&"topFn"));
    assert!(names.contains(&"arrowFn"));
    assert!(names.contains(&"Shape"));
    assert!(names.contains(&"Color"));
    assert!(names.contains(&"Rectangle.area"));
    assert!(
        chunks
            .iter()
            .all(|c| c.strategy == ChunkStrategyTag::AstSymbolTypeScript),
        "{:?}",
        chunks.iter().map(|c| &c.strategy).collect::<Vec<_>>()
    );
}

#[test]
fn js_top_export_arrow_and_class_methods() {
    let text = fixture("sample.js");
    let chunks = chunk_autodetect(&PathBuf::from("sample.js"), &text);
    let names: Vec<_> = chunks
        .iter()
        .filter_map(|c| c.symbol_name.as_deref())
        .collect();
    assert!(names.contains(&"topFn"));
    assert!(names.contains(&"arrowFn"));
    assert!(names.contains(&"Vehicle.constructor"));
    assert!(names.contains(&"Vehicle.describe"));
    assert!(chunks
        .iter()
        .all(|c| c.strategy == ChunkStrategyTag::AstSymbolJavaScript));
}

#[test]
fn tsx_uses_tsx_grammar_strategy() {
    let text = "export function App() { return null; }";
    let chunks = chunk_autodetect(&PathBuf::from("App.tsx"), text);
    assert!(!chunks.is_empty());
    assert!(chunks
        .iter()
        .all(|c| c.strategy == ChunkStrategyTag::AstSymbolTypeScript));
}

#[test]
fn parse_fail_fallback_scanner_rust() {
    let text = "fn broken( { let x = ;";
    let chunks = chunk_autodetect(&PathBuf::from("broken.rs"), text);
    assert!(!chunks.is_empty(), "fallback must produce chunks");
    assert!(chunks.iter().all(|c| c.strategy == ChunkStrategyTag::Scanner));
}

#[test]
fn oversized_rust_splits_with_shared_symbol_name() {
    let many_lets: String = (0..300).map(|i| format!("    let var_{i} = {i};\n")).collect();
    let source = format!("fn big_fn() {{\n{many_lets}}}\n");
    let chunks = chunk_autodetect(&PathBuf::from("big.rs"), &source);
    assert!(
        chunks.len() > 1,
        "expected splits, len={}",
        chunks.len()
    );
    for c in &chunks {
        assert_eq!(c.symbol_name.as_deref(), Some("big_fn"));
        assert_eq!(c.strategy, ChunkStrategyTag::AstSymbolRust);
    }
}

#[test]
fn tiny_fn_merges_undersized() {
    let mut body_b = String::from("    let mut t = x;\n");
    for i in 0..50 {
        body_b.push_str(&format!("    t = t.wrapping_add({i});\n"));
    }
    body_b.push_str("    t\n");
    let source = format!("fn b(x: u32) -> u32 {{\n{body_b}}}\n");
    let chunks = chunk_autodetect(&PathBuf::from("merge.rs"), &source);
    assert_eq!(chunks.len(), 1, "oversized single symbol should coalesce to one named chunk");
}
