// AllMiniLML6V2 max input: 256 tokens. ~1.3 tokens/word → 180 words ≈ 234 tokens.
const CHUNK_WORDS: usize = 180;
const OVERLAP_WORDS: usize = 20;

pub fn chunk_text(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + CHUNK_WORDS).min(words.len());
        let chunk = words[start..end].join(" ");
        if !chunk.trim().is_empty() {
            chunks.push(chunk);
        }
        if end == words.len() {
            break;
        }
        start = end.saturating_sub(OVERLAP_WORDS);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_text_returns_no_chunks() {
        assert!(chunk_text("").is_empty());
        assert!(chunk_text("   ").is_empty());
    }

    #[test]
    fn test_short_text_is_single_chunk() {
        let chunks = chunk_text("hello world this is a short sentence");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello world this is a short sentence");
    }

    #[test]
    fn test_long_text_splits_into_multiple_chunks() {
        let word = "word ";
        let text = word.repeat(1100);
        let chunks = chunk_text(&text);
        assert!(chunks.len() >= 2, "expected multiple chunks, got {}", chunks.len());
    }

    #[test]
    fn test_chunks_have_overlap() {
        // 200 words → chunk1: 0..179, chunk2: 160..199 → exactly 2 chunks
        let words: Vec<String> = (0..200).map(|i| format!("w{}", i)).collect();
        let text = words.join(" ");
        let chunks = chunk_text(&text);
        assert_eq!(chunks.len(), 2);
        let last_of_first: Vec<&str> = chunks[0].split_whitespace().rev().take(OVERLAP_WORDS).collect::<Vec<_>>().into_iter().rev().collect();
        let first_of_second: Vec<&str> = chunks[1].split_whitespace().take(OVERLAP_WORDS).collect();
        assert_eq!(last_of_first, first_of_second);
    }

    #[test]
    fn test_chunk_max_word_count() {
        let text = "word ".repeat(1200);
        let chunks = chunk_text(&text);
        for chunk in &chunks {
            let word_count = chunk.split_whitespace().count();
            assert!(word_count <= CHUNK_WORDS, "chunk has {} words", word_count);
        }
    }
}
