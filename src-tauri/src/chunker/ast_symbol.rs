//! Tree-sitter symbol chunking per `docs/superpowers/specs/2026-05-07-ast-symbol-chunking-design.md`.

use std::collections::HashMap;

use tree_sitter::{
    Language, Node, Parser, Query, QueryCapture, QueryCursor, StreamingIterator, Tree,
};

use super::classify::AstLang;
use super::code_like::statement_boundary_offsets;
use super::{Chunk, ChunkStrategyTag};

const RUST_QUERY: &str = r#"
(function_item name: (identifier) @name) @symbol
(struct_item name: (type_identifier) @name) @symbol
(enum_item name: (type_identifier) @name) @symbol
(trait_item name: (type_identifier) @name) @symbol
(type_item name: (type_identifier) @name) @symbol
"#;

const TS_QUERY: &str = r#"
(program (function_declaration name: (identifier) @name) @symbol)
(export_statement declaration: (function_declaration name: (identifier) @name) @symbol)
(interface_declaration name: (type_identifier) @name) @symbol
(type_alias_declaration name: (type_identifier) @name) @symbol
(export_statement declaration: (lexical_declaration
  (variable_declarator name: (identifier) @name))) @symbol
(class_declaration name: (type_identifier) @name
  body: (class_body (method_definition name: (property_identifier) @name) @symbol))
"#;

const JS_QUERY: &str = r#"
(program (function_declaration name: (identifier) @name) @symbol)
(export_statement declaration: (function_declaration name: (identifier) @name) @symbol)
(export_statement declaration: (lexical_declaration
  (variable_declarator name: (identifier) @name))) @symbol
(class_declaration name: (identifier) @name
  body: (class_body (method_definition name: (property_identifier) @name) @symbol))
"#;

const PYTHON_TOP_FN: &str = r#"
(module (function_definition name: (identifier) @name) @symbol)
"#;

const PYTHON_CLASS_METHOD: &str = r#"
(class_definition name: (identifier) @classname
  body: (block (function_definition name: (identifier) @name) @symbol))
"#;

const GO_QUERY: &str = r#"
(function_declaration name: (identifier) @name) @symbol
(method_declaration name: (field_identifier) @name) @symbol
(type_declaration (type_spec name: (type_identifier) @name)) @symbol
"#;

const MAX_WORDS: usize = 400;
const MIN_WORDS: usize = 20;

pub(super) fn chunk(lang: AstLang, text: &str, use_tsx: bool) -> Vec<Chunk> {
    try_chunk(lang, text, use_tsx)
        .filter(|v| !v.is_empty())
        .unwrap_or_default()
}

fn try_chunk(lang: AstLang, text: &str, use_tsx: bool) -> Option<Vec<Chunk>> {
    let lang_ref = ts_language(lang, use_tsx);
    let mut parser = Parser::new();
    parser.set_language(&lang_ref).ok()?;
    let tree = parser.parse(text, None)?;
    let tag = strategy_tag(lang);
    let raw = match lang {
        AstLang::Rust => collect_rust(&lang_ref, &tree, text, tag)?,
        AstLang::Python => collect_python(&lang_ref, &tree, text, tag)?,
        AstLang::Go => collect_go(&lang_ref, &tree, text, tag)?,
        AstLang::TypeScript => collect_ts_js(&lang_ref, TS_QUERY, &tree, text, tag)?,
        AstLang::JavaScript => collect_ts_js(&lang_ref, JS_QUERY, &tree, text, tag)?,
    };
    Some(post_process(raw))
}

fn ts_language(lang: AstLang, use_tsx: bool) -> Language {
    match lang {
        AstLang::Rust => tree_sitter_rust::LANGUAGE.into(),
        AstLang::TypeScript => {
            if use_tsx {
                tree_sitter_typescript::LANGUAGE_TSX.into()
            } else {
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
            }
        }
        AstLang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        AstLang::Python => tree_sitter_python::LANGUAGE.into(),
        AstLang::Go => tree_sitter_go::LANGUAGE.into(),
    }
}

fn strategy_tag(lang: AstLang) -> ChunkStrategyTag {
    match lang {
        AstLang::Rust => ChunkStrategyTag::AstSymbolRust,
        AstLang::TypeScript => ChunkStrategyTag::AstSymbolTypeScript,
        AstLang::JavaScript => ChunkStrategyTag::AstSymbolJavaScript,
        AstLang::Python => ChunkStrategyTag::AstSymbolPython,
        AstLang::Go => ChunkStrategyTag::AstSymbolGo,
    }
}

fn query_named(lang: &Language, pattern: &str) -> Option<Query> {
    Query::new(lang, pattern).ok()
}

/// Returns rows of `(symbol_node, name_node, optional @classname)`.
fn run_query<'a>(
    query: &'a Query,
    tree: &'a Tree,
    source: &'a [u8],
) -> Vec<(Node<'a>, Node<'a>, Option<Node<'a>>)> {
    let symbol_idx = query.capture_index_for_name("symbol");
    let name_idx = query.capture_index_for_name("name");
    let class_idx = query.capture_index_for_name("classname");
    let (Some(symbol_idx), Some(name_idx)) = (symbol_idx, name_idx) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut mq = cursor.matches(query, tree.root_node(), source);
    loop {
        mq.advance();
        let Some(mat) = mq.get() else {
            break;
        };
        let mut sym_n: Option<Node<'a>> = None;
        let mut name_n: Option<Node<'a>> = None;
        let mut class_n: Option<Node<'a>> = None;
        for cap in mat.captures {
            let QueryCapture { node, index, .. } = *cap;
            if index == symbol_idx {
                sym_n = Some(node);
            } else if index == name_idx {
                name_n = Some(node);
            } else if class_idx == Some(index) {
                class_n = Some(node);
            }
        }
        if let (Some(s), Some(n)) = (sym_n, name_n) {
            out.push((s, n, class_n));
        }
    }
    out
}

fn nt<'a>(text: &'a str, n: Node) -> Option<&'a str> {
    n.utf8_text(text.as_bytes()).ok()
}

/// Top-level or `impl` method `function_item`; not a nested inner function.
fn rust_function_item_keep(sym: Node<'_>) -> bool {
    if impl_parent_of_method(sym).is_some() {
        return true;
    }
    let mut cur = sym.parent();
    while let Some(p) = cur {
        if p.kind() == "source_file" {
            return true;
        }
        if p.kind() == "function_item" {
            return false;
        }
        cur = p.parent();
    }
    false
}

fn impl_parent_of_method(sym: Node<'_>) -> Option<Node<'_>> {
    let decl = sym.parent().filter(|n| n.kind() == "declaration_list")?;
    let impl_item = decl.parent().filter(|n| n.kind() == "impl_item")?;
    Some(impl_item)
}

fn rust_impl_preamble(impl_item: Node<'_>, text: &str, first_method: Node<'_>) -> Option<String> {
    let list = impl_item.child_by_field_name("body")?;
    if list.kind() != "declaration_list" {
        return None;
    }
    let mut preamble = String::new();
    let mut w = list.walk();
    for ch in list.named_children(&mut w) {
        if ch.start_byte() >= first_method.start_byte() {
            break;
        }
        if ch.kind() == "function_item" {
            continue;
        }
        let slice = text.get(ch.start_byte()..ch.end_byte())?;
        if !preamble.is_empty() {
            preamble.push('\n');
        }
        preamble.push_str(slice.trim());
    }
    if preamble.is_empty() {
        None
    } else {
        Some(preamble)
    }
}

fn collect_rust(
    lang: &Language,
    tree: &Tree,
    text: &str,
    tag: ChunkStrategyTag,
) -> Option<Vec<Chunk>> {
    let q = query_named(lang, RUST_QUERY)?;
    let rows = run_query(&q, tree, text.as_bytes());
    let mut first_method: HashMap<usize, Node<'_>> = HashMap::new();
    for (sym, _, _) in &rows {
        if sym.kind() != "function_item" {
            continue;
        }
        if let Some(im) = impl_parent_of_method(*sym) {
            let id = im.id();
            first_method
                .entry(id)
                .and_modify(|n| {
                    if sym.start_byte() < n.start_byte() {
                        *n = *sym;
                    }
                })
                .or_insert(*sym);
        }
    }

    let mut out: Vec<Chunk> = Vec::new();
    for (sym, name_node, _) in rows {
        if sym.kind() == "function_item" && !rust_function_item_keep(sym) {
            continue;
        }
        let raw_name = nt(text, name_node)?;
        let sym_text = nt(text, sym)?;
        let start_line = sym.start_position().row as u32 + 1;
        let end_line = sym.end_position().row as u32 + 1;
        let (symbol_name_opt, mut body) = qualify_rust_fn(sym, raw_name, sym_text, text)?;

        if sym.kind() == "function_item" {
            if let Some(im) = impl_parent_of_method(sym) {
                if first_method
                    .get(&im.id())
                    .is_some_and(|fm| fm.id() == sym.id())
                {
                    if let Some(pre) = rust_impl_preamble(im, text, sym) {
                        body = format!("{pre}\n{body}");
                    }
                }
            }
        }

        let defined_symbol = Some(
            symbol_name_opt
                .as_deref()
                .unwrap_or(raw_name)
                .to_lowercase(),
        );
        out.push(Chunk {
            text: body,
            start_line,
            end_line,
            symbol_name: symbol_name_opt.or_else(|| Some(raw_name.to_string())),
            defined_symbol,
            strategy: tag.clone(),
        });
    }

    out.sort_by_key(|c| (c.start_line, c.end_line));
    Some(out)
}

fn qualify_rust_fn(
    sym: Node<'_>,
    raw_name: &str,
    sym_text: &str,
    text: &str,
) -> Option<(Option<String>, String)> {
    if sym.kind() == "function_item" {
        if let Some(impl_n) = impl_parent_of_method(sym) {
            let type_node = impl_n.child_by_field_name("type")?;
            let ty = type_node.utf8_text(text.as_bytes()).ok()?.trim();
            if ty.is_empty() {
                return Some((None, format!("// impl\n{}", sym_text)));
            }
            let qualified = format!("{}::{}", ty, raw_name);
            let prefix = format!("// impl {}", ty);
            return Some((Some(qualified), format!("{prefix}\n{sym_text}")));
        }
    }
    Some((Some(raw_name.to_string()), sym_text.to_string()))
}

fn python_fn_top_level(sym: Node<'_>) -> bool {
    let mut cur = sym.parent();
    while let Some(p) = cur {
        if p.kind() == "function_definition" {
            return false;
        }
        if p.kind() == "module" {
            return true;
        }
        cur = p.parent();
    }
    false
}

fn python_class_has_method(class: Node<'_>) -> bool {
    let mut w = class.walk();
    for ch in class.named_children(&mut w) {
        if ch.kind() != "block" {
            continue;
        }
        let mut w2 = ch.walk();
        for it in ch.named_children(&mut w2) {
            if it.kind() == "function_definition" {
                return true;
            }
        }
    }
    false
}

fn collect_python(
    lang: &Language,
    tree: &Tree,
    text: &str,
    tag: ChunkStrategyTag,
) -> Option<Vec<Chunk>> {
    let mut out: Vec<Chunk> = Vec::new();
    let q_top = query_named(lang, PYTHON_TOP_FN)?;
    for (sym, nm, _) in run_query(&q_top, tree, text.as_bytes()) {
        if !python_fn_top_level(sym) {
            continue;
        }
        let raw_name = nt(text, nm)?;
        let sym_text = nt(text, sym)?;
        out.push(chunk_simple(
            sym_text.to_string(),
            sym,
            Some(raw_name.to_string()),
            &tag,
        ));
    }

    let q_m = query_named(lang, PYTHON_CLASS_METHOD)?;
    for (sym, nm, cls) in run_query(&q_m, tree, text.as_bytes()) {
        let method = nt(text, nm)?;
        let class_name = nt(text, cls?)?;
        let sym_text = nt(text, sym)?;
        let qualified = format!("{}.{}", class_name, method);
        let body = format!("# class {}\n{}", class_name, sym_text);
        out.push(chunk_simple(body, sym, Some(qualified), &tag));
    }

    let root = tree.root_node();
    let mut w = root.walk();
    for ch in root.named_children(&mut w) {
        if ch.kind() != "class_definition" {
            continue;
        }
        if python_class_has_method(ch) {
            continue;
        }
        let name_n = ch.child_by_field_name("name")?;
        let raw_name = nt(text, name_n)?;
        let sym_text = nt(text, ch)?;
        out.push(chunk_simple(
            sym_text.to_string(),
            ch,
            Some(raw_name.to_string()),
            &tag,
        ));
    }

    out.sort_by_key(|c| (c.start_line, c.end_line));
    Some(out)
}

fn chunk_simple(
    text: String,
    sym: Node<'_>,
    name: Option<String>,
    tag: &ChunkStrategyTag,
) -> Chunk {
    let defined_symbol = name.as_ref().map(|s| s.to_lowercase());
    Chunk {
        text,
        start_line: sym.start_position().row as u32 + 1,
        end_line: sym.end_position().row as u32 + 1,
        symbol_name: name,
        defined_symbol,
        strategy: tag.clone(),
    }
}

fn go_method_qualify(
    method: Node<'_>,
    method_name: &str,
    sym_text: &str,
    text: &str,
) -> Option<(String, String)> {
    let receiver = method.child_by_field_name("receiver")?;
    let pd = receiver.named_child(0)?;
    let type_node = pd.child_by_field_name("type")?;
    let type_src = type_node.utf8_text(text.as_bytes()).ok()?.trim();
    let is_ptr = type_src.starts_with('*');
    let bare = type_src.trim_start_matches('*').trim();
    let q = if is_ptr {
        format!("(*{}).{}", bare, method_name)
    } else {
        format!("{}.{}", bare, method_name)
    };
    let prefix = format!("// type {}", bare);
    Some((q, format!("{}\n{}", prefix, sym_text)))
}

fn collect_go(
    lang: &Language,
    tree: &Tree,
    text: &str,
    tag: ChunkStrategyTag,
) -> Option<Vec<Chunk>> {
    let q = query_named(lang, GO_QUERY)?;
    let mut out = Vec::new();
    for (sym, nm, _) in run_query(&q, tree, text.as_bytes()) {
        let id = nt(text, nm)?;
        let sym_text = nt(text, sym)?;
        let start_line = sym.start_position().row as u32 + 1;
        let end_line = sym.end_position().row as u32 + 1;
        let (qualified, body) = if sym.kind() == "method_declaration" {
            match go_method_qualify(sym, id, sym_text, text) {
                Some(pair) => (Some(pair.0), pair.1),
                None => (Some(id.to_string()), sym_text.to_string()),
            }
        } else {
            (Some(id.to_string()), sym_text.to_string())
        };

        let defined_symbol = qualified.as_ref().map(|s| s.to_lowercase());
        out.push(Chunk {
            text: body,
            start_line,
            end_line,
            symbol_name: qualified,
            defined_symbol,
            strategy: tag.clone(),
        });
    }
    out.sort_by_key(|c| (c.start_line, c.end_line));
    Some(out)
}

fn collect_ts_js(
    lang_ref: &Language,
    pattern: &str,
    tree: &Tree,
    text: &str,
    tag: ChunkStrategyTag,
) -> Option<Vec<Chunk>> {
    let q = query_named(lang_ref, pattern)?;
    let mut out = Vec::new();
    for (sym, nm, _) in run_query(&q, tree, text.as_bytes()) {
        let id = nt(text, nm)?;
        let sym_text = nt(text, sym)?;
        if sym.kind() == "function_declaration" && !tsjs_function_decl_top_level(sym) {
            continue;
        }
        let (qual, body) = if sym.kind() == "method_definition" {
            qualify_ts_method(sym, id, sym_text, text)?
        } else {
            (id.to_string(), sym_text.to_string())
        };
        out.push(chunk_simple(body, sym, Some(qual), &tag));
    }
    out.sort_by_key(|c| (c.start_line, c.end_line));
    Some(out)
}

fn tsjs_function_decl_top_level(sym: Node<'_>) -> bool {
    let mut cur = sym.parent();
    while let Some(p) = cur {
        if p.kind() == "function_declaration" {
            return false;
        }
        if p.kind() == "program" {
            return true;
        }
        cur = p.parent();
    }
    false
}

fn qualify_ts_method(
    sym: Node<'_>,
    raw_name: &str,
    sym_text: &str,
    text: &str,
) -> Option<(String, String)> {
    let cb = sym.parent().filter(|n| n.kind() == "class_body")?;
    let class_decl = cb.parent().filter(|n| n.kind() == "class_declaration")?;
    let nm = class_decl.child_by_field_name("name")?;
    let class_name = nm.utf8_text(text.as_bytes()).ok()?;
    let q = format!("{}.{}", class_name, raw_name);
    let px = format!("// class {}", class_name);
    Some((q, format!("{}\n{}", px, sym_text)))
}

fn post_process(mut chunks: Vec<Chunk>) -> Vec<Chunk> {
    let mut acc = Vec::new();
    for c in chunks.drain(..) {
        acc.extend(split_oversized_ast(c));
    }
    merge_undersized(acc)
}

fn lang_for_split(strategy: ChunkStrategyTag) -> Option<(AstLang, bool)> {
    match strategy {
        ChunkStrategyTag::AstSymbolRust => Some((AstLang::Rust, false)),
        ChunkStrategyTag::AstSymbolTypeScript => Some((AstLang::TypeScript, true)),
        ChunkStrategyTag::AstSymbolJavaScript => Some((AstLang::JavaScript, false)),
        ChunkStrategyTag::AstSymbolPython => Some((AstLang::Python, false)),
        ChunkStrategyTag::AstSymbolGo => Some((AstLang::Go, false)),
        _ => None,
    }
}

fn split_maybe_oversized_no_inner(chunk: Chunk) -> Vec<Chunk> {
    let mut g = greedy_stmt_split(chunk.clone());
    if g.len() == 1 && word_count(&g[0].text) > MAX_WORDS {
        return split_fallback_newline_words_chunk(g.pop().expect("len 1"));
    }
    g
}

/// Line-bounded splitting when braces give no usable statement boundaries (spec §6).
fn split_fallback_newline_words_chunk(chunk: Chunk) -> Vec<Chunk> {
    let full = chunk.text.as_str();
    let base = chunk.start_line;
    if full.is_empty() {
        return vec![chunk];
    }
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut a = 0usize;
    while a < full.len() {
        let b = full[a..]
            .find('\n')
            .map(|i| a + i + 1)
            .unwrap_or(full.len());
        spans.push((a, b));
        if b == full.len() {
            break;
        }
        a = b;
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < spans.len() {
        let (lo0, _) = spans[i];
        let mut hi = spans[i].1;
        let mut wc = word_count(trim_segment(full, lo0, hi));
        let mut j = i;
        while j + 1 < spans.len() {
            let (nl, nh) = spans[j + 1];
            let add = word_count(trim_segment(full, nl, nh));
            if wc > 0 && wc + add > MAX_WORDS {
                break;
            }
            wc += add;
            hi = nh;
            j += 1;
        }
        push_trimmed_chunk_slice(full, lo0, hi, base, &chunk, &mut out);
        i = j + 1;
    }
    if out.len() <= 1 {
        vec![chunk]
    } else {
        out
    }
}

fn split_oversized_ast(chunk: Chunk) -> Vec<Chunk> {
    if word_count(&chunk.text) <= MAX_WORDS {
        return vec![chunk];
    }
    let Some((al, tsx)) = lang_for_split(chunk.strategy.clone()) else {
        return greedy_stmt_split(chunk);
    };
    let lang_o = ts_language(al, tsx);
    let mut parser = Parser::new();
    if parser.set_language(&lang_o).is_err() {
        return greedy_stmt_split(chunk);
    }
    let Some(tree) = parser.parse(&chunk.text, None) else {
        return greedy_stmt_split(chunk);
    };
    let root = tree.root_node();
    let kinds: &[&str] = match al {
        AstLang::Rust => &["function_item"],
        AstLang::Python => &["function_definition"],
        AstLang::Go => &["function_declaration", "method_declaration"],
        AstLang::TypeScript | AstLang::JavaScript => &["function_declaration", "arrow_function"],
    };
    let anchor = parse_anchor_for_split(al, root);
    let inner = inner_spans_within_anchor(anchor, kinds);

    let base_sl = chunk.start_line;
    if inner.is_empty() {
        return split_maybe_oversized_no_inner(chunk);
    }

    let mut pieces: Vec<Chunk> = Vec::new();
    let body = chunk.text.as_str();
    let mut cursor = 0usize;
    for (lo, hi) in inner {
        if lo > cursor {
            pieces.extend(subchunk_trimmed_slice(body, base_sl, cursor, lo, &chunk));
        }
        if let Some(seg) = body.get(lo..hi) {
            let trimmed = seg.trim();
            if !trimmed.is_empty() {
                let off = seg.find(trimmed).unwrap_or(0) + lo;
                let sl = line_in_symbol(body, base_sl, off);
                let el = line_in_symbol(body, base_sl, hi.saturating_sub(1));
                pieces.push(Chunk {
                    text: trimmed.to_string(),
                    start_line: sl,
                    end_line: el.max(sl),
                    symbol_name: chunk.symbol_name.clone(),
                    defined_symbol: chunk.defined_symbol.clone(),
                    strategy: chunk.strategy.clone(),
                });
            }
        }
        cursor = hi;
    }
    if cursor < body.len() {
        pieces.extend(subchunk_trimmed_slice(
            body,
            base_sl,
            cursor,
            body.len(),
            &chunk,
        ));
    }

    if pieces.len() <= 1 {
        split_maybe_oversized_no_inner(chunk)
    } else {
        pieces
    }
}

fn line_in_symbol(full_sym: &str, sym_start_line: u32, byte_off: usize) -> u32 {
    let upto = byte_off.min(full_sym.len());
    let lines = full_sym[..upto].bytes().filter(|b| *b == b'\n').count() as u32;
    sym_start_line + lines
}

fn parse_anchor_for_split(al: AstLang, root: Node<'_>) -> Node<'_> {
    match al {
        AstLang::Rust | AstLang::JavaScript => {
            if matches!(root.kind(), "source_file" | "program") && root.named_child_count() == 1 {
                return root.named_child(0).unwrap_or(root);
            }
        }
        AstLang::TypeScript => {
            if root.kind() == "program" && root.named_child_count() == 1 {
                return root.named_child(0).unwrap_or(root);
            }
        }
        AstLang::Python => {
            if root.kind() == "module" && root.named_child_count() == 1 {
                return root.named_child(0).unwrap_or(root);
            }
        }
        AstLang::Go => {
            if root.kind() == "source_file" && root.named_child_count() == 1 {
                return root.named_child(0).unwrap_or(root);
            }
        }
    }
    root
}

fn inner_spans_within_anchor(anchor: Node<'_>, kinds: &[&str]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    collect_inner_recursive(anchor, anchor, kinds, &mut out);
    out.sort_by_key(|(a, _)| *a);
    out.dedup();
    out
}

fn collect_inner_recursive(
    anchor: Node<'_>,
    node: Node<'_>,
    kinds: &[&str],
    out: &mut Vec<(usize, usize)>,
) {
    if node.id() != anchor.id() && kinds.contains(&node.kind()) {
        let a = anchor.start_byte();
        let b = anchor.end_byte();
        if node.start_byte() >= a && node.end_byte() <= b {
            out.push((node.start_byte(), node.end_byte()));
        }
    }
    for i in 0..node.named_child_count() {
        if let Some(ch) = node.named_child(i as u32) {
            collect_inner_recursive(anchor, ch, kinds, out);
        }
    }
}

fn subchunk_trimmed_slice(
    full: &str,
    base_sl: u32,
    lo: usize,
    hi: usize,
    parent: &Chunk,
) -> Vec<Chunk> {
    let raw = full.get(lo..hi).unwrap_or("");
    let sub = raw.trim();
    if sub.is_empty() {
        return Vec::new();
    }
    let inner_off = raw.find(sub).unwrap_or(0);
    let abs_lo = lo + inner_off;
    let abs_hi = abs_lo + sub.len();
    greedy_stmt_split(Chunk {
        text: sub.to_string(),
        start_line: line_in_symbol(full, base_sl, abs_lo),
        end_line: line_in_symbol(full, base_sl, abs_hi.saturating_sub(1)),
        symbol_name: parent.symbol_name.clone(),
        defined_symbol: parent.defined_symbol.clone(),
        strategy: parent.strategy.clone(),
    })
}

fn greedy_stmt_split(chunk: Chunk) -> Vec<Chunk> {
    if word_count(&chunk.text) <= MAX_WORDS {
        return vec![chunk];
    }
    let s = chunk.text.as_str();
    let base = chunk.start_line;
    let mut cuts: Vec<usize> = vec![0];
    cuts.extend(
        statement_boundary_offsets(s)
            .into_iter()
            .filter(|&p| p > 0 && p <= s.len()),
    );
    cuts.push(s.len());
    cuts.sort_unstable();
    cuts.dedup();

    let segments: Vec<(usize, usize)> = cuts.windows(2).map(|w| (w[0], w[1])).collect();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < segments.len() {
        let (lo, mut hi) = segments[i];
        let mut wc = word_count(trim_segment(s, lo, hi));
        let mut j = i;
        while j + 1 < segments.len() {
            let (nl, nh) = segments[j + 1];
            let add = word_count(trim_segment(s, nl, nh));
            if wc > 0 && wc + add > MAX_WORDS {
                break;
            }
            wc += add;
            hi = nh;
            j += 1;
        }
        push_trimmed_chunk_slice(s, lo, hi, base, &chunk, &mut out);
        i = j + 1;
    }

    if out.is_empty() {
        vec![chunk]
    } else {
        out
    }
}

fn trim_segment(full: &str, lo: usize, hi: usize) -> &str {
    full.get(lo..hi).unwrap_or("").trim()
}

fn push_trimmed_chunk_slice(
    full: &str,
    lo: usize,
    hi: usize,
    base_sl: u32,
    parent: &Chunk,
    out: &mut Vec<Chunk>,
) {
    let raw = full.get(lo..hi).unwrap_or("");
    let piece = raw.trim();
    if piece.is_empty() {
        return;
    }
    let off = raw.find(piece).unwrap_or(0) + lo;
    let end_b = off + piece.len();
    let sl = line_in_symbol(full, base_sl, off);
    let el = line_in_symbol(full, base_sl, end_b.saturating_sub(1));
    out.push(Chunk {
        text: piece.to_string(),
        start_line: sl,
        end_line: el.max(sl),
        symbol_name: parent.symbol_name.clone(),
        defined_symbol: parent.defined_symbol.clone(),
        strategy: parent.strategy.clone(),
    });
}

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

fn merge_undersized(mut chunks: Vec<Chunk>) -> Vec<Chunk> {
    if chunks.len() < 2 {
        return chunks;
    }
    let mut i = 0usize;
    while i < chunks.len() {
        if word_count(&chunks[i].text) >= MIN_WORDS {
            i += 1;
            continue;
        }
        let can_fwd = i + 1 < chunks.len() && chunks[i].symbol_name == chunks[i + 1].symbol_name;
        let can_back = i > 0 && chunks[i].symbol_name == chunks[i - 1].symbol_name;
        if can_fwd {
            let tiny = chunks.remove(i);
            chunks[i].text = format!("{}\n\n{}", tiny.text, chunks[i].text);
            chunks[i].start_line = tiny.start_line;
        } else if can_back {
            let tiny = chunks.remove(i);
            chunks[i - 1].text = format!("{}\n\n{}", chunks[i - 1].text, tiny.text);
            chunks[i - 1].end_line = tiny.end_line;
        } else {
            i += 1;
        }
    }
    chunks
}
