//! Stable content-derived chunk identifier.
//!
//! SHA-256(text || doc_path || position_le_u64), first 16 bytes as 32 hex chars.
//!
//! **Determinism guard:** The hash is stable only if the chunker enumerates chunks
//! in the same order for a given input. Today's chunker strategies (AstSymbol,
//! Prose, CodeLike, Declarative, Fallback — see `src-tauri/src/chunker/mod.rs`)
//! iterate the AST/text in source order and emit chunks in iteration order; the
//! pipeline hash short-circuit (`pipeline/mod.rs:518-520`) guarantees chunks are
//! only re-inserted when the file changes, so position is reliable on stable
//! content. If the chunker is ever changed to non-deterministic chunk-ordering
//! (parallel, set-based, etc.) the hash strategy breaks SILENTLY — re-chunks
//! will hash to different values and every fact's `content_hash` will orphan.
//! Refactor with care and add a regression test if you change chunk ordering.

use sha2::{Digest, Sha256};

pub fn compute_chunk_hash(text: &str, doc_path: &str, position: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher.update(doc_path.as_bytes());
    hasher.update((position as u64).to_le_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(32);
    for byte in &digest[..16] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_chunk_hash_is_deterministic_for_same_input() {
        let h1 = compute_chunk_hash("hello", "documents/a.md", 0);
        let h2 = compute_chunk_hash("hello", "documents/a.md", 0);
        assert_eq!(h1, h2);
    }

    #[test]
    fn compute_chunk_hash_differs_on_text_change() {
        let h1 = compute_chunk_hash("hello", "documents/a.md", 0);
        let h2 = compute_chunk_hash("HELLO", "documents/a.md", 0);
        assert_ne!(h1, h2);
    }

    #[test]
    fn compute_chunk_hash_differs_on_position_change() {
        let h1 = compute_chunk_hash("hello", "documents/a.md", 0);
        let h2 = compute_chunk_hash("hello", "documents/a.md", 1);
        assert_ne!(
            h1, h2,
            "position tie-break must avoid duplicate-chunk collisions"
        );
    }

    #[test]
    fn compute_chunk_hash_differs_on_path_change() {
        let h1 = compute_chunk_hash("hello", "documents/a.md", 0);
        let h2 = compute_chunk_hash("hello", "documents/b.md", 0);
        assert_ne!(h1, h2, "cross-doc collisions must not share a hash");
    }

    #[test]
    fn compute_chunk_hash_returns_32_hex_chars() {
        let h = compute_chunk_hash("hello", "documents/a.md", 0);
        assert_eq!(h.len(), 32, "first 16 bytes of SHA-256 = 32 hex chars");
        assert!(
            h.chars().all(|c| c.is_ascii_hexdigit()),
            "must be lowercase hex"
        );
        assert!(h.chars().all(|c| !c.is_ascii_uppercase()), "lowercase only");
    }
}
