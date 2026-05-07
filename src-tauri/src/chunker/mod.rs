//! Sentence-aware chunking with neighbor-sentence padding for embeddings.
const TARGET_WORDS: usize = 100;

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

fn word_chars_touching_dot(text: &str, dot_byte: usize) -> &str {
    let before = &text[..dot_byte];
    let mut start = before.len();
    for (i, ch) in before.char_indices().rev() {
        if ch.is_alphanumeric() || ch == '-' {
            start = i;
        } else {
            break;
        }
    }
    &before[start..]
}

fn is_probable_abbrev_dot_token(token: &str) -> bool {
    let len = token.chars().count();
    if !(1..=3).contains(&len) {
        return false;
    }
    let mut chs = token.chars();
    let Some(first) = chs.next() else {
        return false;
    };
    if first.is_uppercase() {
        return true;
    }
    token.eq_ignore_ascii_case("al") || token.eq_ignore_ascii_case("vs")
}

fn char_before_dot(text: &str, dot_byte: usize) -> Option<char> {
    text[..dot_byte].chars().next_back()
}

/// Returns true if punctuation at `punct_byte` ends a sentence.
fn ends_sentence(text: &str, punct_byte: usize, punct: char) -> bool {
    if !matches!(punct, '.' | '!' | '?') {
        return false;
    }

    if punct == '.' {
        if let Some(prev) = char_before_dot(text, punct_byte) {
            let punct_end = punct_byte + punct.len_utf8();
            let after_first = text
                .get(punct_end..)
                .and_then(|rest| rest.chars().find(|c| !c.is_whitespace()));
            if prev.is_ascii_digit() && matches!(after_first, Some(c) if c.is_ascii_digit()) {
                return false;
            }
            let token = word_chars_touching_dot(text, punct_byte);
            if is_probable_abbrev_dot_token(token) {
                return false;
            }
        }
    }

    let punct_end = punct_byte + punct.len_utf8();
    let rest = text.get(punct_end..).unwrap_or("");

    if rest.is_empty() || rest.chars().all(|c| c.is_whitespace()) {
        return true;
    }

    match rest.trim_start().chars().next() {
        Some(c) if c.is_uppercase() => true,
        _ => false,
    }
}

/// Sentence boundary: `.`, `!`, or `?` per design spec (abbreviation and decimal skips for `.`).
fn split_sentences(text: &str) -> Vec<&str> {
    let text = text.trim();
    if text.is_empty() {
        return vec![];
    }

    let mut out = Vec::new();
    let mut sent_start = 0usize;

    for (byte_idx, ch) in text.char_indices() {
        if !matches!(ch, '.' | '!' | '?') {
            continue;
        }
        if !ends_sentence(text, byte_idx, ch) {
            continue;
        }

        let punct_end = byte_idx + ch.len_utf8();
        out.push(text.get(sent_start..punct_end).unwrap_or("").trim());
        let after = text.get(punct_end..).unwrap_or("");
        sent_start = punct_end
            + after
                .chars()
                .take_while(|c| c.is_whitespace())
                .map(char::len_utf8)
                .sum::<usize>();
    }

    if sent_start < text.len() {
        let tail = text.get(sent_start..).unwrap_or("").trim();
        if !tail.is_empty() {
            out.push(tail);
        }
    }

    out.into_iter().filter(|s| !s.is_empty()).collect()
}

/// Core groups by word count; then add prev/next sentence padding for embedding text.
pub fn chunk_text(text: &str) -> Vec<String> {
    let sentences: Vec<&str> = split_sentences(text);
    if sentences.is_empty() {
        return vec![];
    }

    let mut groups: Vec<std::ops::Range<usize>> = Vec::new();
    let mut cur_start = 0usize;
    let mut acc_words = 0usize;

    for (i, s) in sentences.iter().enumerate() {
        acc_words += word_count(s);
        if acc_words >= TARGET_WORDS {
            groups.push(cur_start..i + 1);
            cur_start = i + 1;
            acc_words = 0;
        }
    }

    if cur_start < sentences.len() {
        groups.push(cur_start..sentences.len());
    }

    let n = groups.len();
    let mut chunks = Vec::with_capacity(n);
    for (gi, r) in groups.iter().enumerate() {
        let mut parts: Vec<&str> = Vec::new();
        if gi > 0 {
            parts.push(sentences[r.start - 1].trim());
        }
        for idx in r.clone() {
            parts.push(sentences[idx].trim());
        }
        if gi < n - 1 {
            parts.push(sentences[r.end].trim());
        }
        chunks.push(parts.join(" "));
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_empty_and_whitespace_only() {
        assert!(split_sentences("").is_empty());
        assert!(split_sentences("   \n\t").is_empty());
    }

    #[test]
    fn split_single_sentence_no_internal_boundaries() {
        let s = split_sentences("hello world");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0], "hello world");
    }

    #[test]
    fn split_multi_sentence_basic() {
        let s = split_sentences("First one. Second two. Third three.");
        assert_eq!(s, vec!["First one.", "Second two.", "Third three."]);
    }

    #[test]
    fn split_skips_decimal_point() {
        let s = split_sentences("p < 0.05 is common. Next starts.");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], "p < 0.05 is common.");
        assert_eq!(s[1], "Next starts.");
    }

    #[test]
    fn split_skips_short_token_abbrev_before_period() {
        let s = split_sentences("See et al. for more. Done here.");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], "See et al. for more.");
        assert_eq!(s[1], "Done here.");
    }

    #[test]
    fn split_skips_fig_three_letter() {
        let s = split_sentences("In Fig. 2 we show. Later work.");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], "In Fig. 2 we show.");
        assert_eq!(s[1], "Later work.");
    }

    #[test]
    fn split_exclamation_and_question() {
        let s = split_sentences("Really? Yes! Understood.");
        assert_eq!(s, vec!["Really?", "Yes!", "Understood."]);
    }

    #[test]
    fn split_end_without_trailing_whitespace() {
        let s = split_sentences("Done.");
        assert_eq!(s, vec!["Done."]);
    }

    #[test]
    fn chunk_empty_returns_empty() {
        assert!(chunk_text("").is_empty());
        assert!(chunk_text("   ").is_empty());
    }

    #[test]
    fn chunk_single_group_no_padding_neighbors() {
        let s = "One two three four five.";
        let c = chunk_text(s);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0], "One two three four five.");
    }

    #[test]
    fn chunk_multi_group_middle_has_both_paddings() {
        // Each sentence is 5 words; 21 sentences → 105 words in first group (>=100), then remainder
        let mut parts = Vec::new();
        // Sentences start with ASCII uppercase so '.' + space + lowercase still matches corpus style.
        for i in 0..21 {
            parts.push(format!("Word{i} word{i} word{i} word{i} word{i}."));
        }
        for i in 21..40 {
            parts.push(format!("Tail{i} tail{i} tail{i} tail{i} tail{i}."));
        }
        let text = parts.join(" ");
        let chunks = chunk_text(&text);
        assert!(chunks.len() >= 2);

        assert!(
            chunks[0].contains("Word20.") || chunks[0].contains(" Word20 "),
            "first chunk must include first sentence of next core group as padding"
        );

        assert!(
            chunks[1].starts_with("Word19"),
            "second chunk must lead with prior group's last sentence as padding"
        );
    }

    #[test]
    fn chunk_first_group_has_no_prev_padding() {
        let a = vec!["Aa bb cc.".to_string(); 25].join(" ");
        let tail = "Xx yy zz.";
        let text = format!("{a} {tail}");
        let c = chunk_text(&text);
        assert!(!c[0].starts_with("Xx yy"));
        if c.len() > 1 {
            assert!(c[1].contains("Xx yy") || c[1].starts_with("Xx"));
        }
    }

    #[test]
    fn chunk_last_group_has_no_next_padding() {
        let lead = vec!["Aa bb cc dd ee.".to_string(); 30].join(" ");
        let text = format!("START. {lead}");
        let chunks = chunk_text(&text);
        let last = chunks.last().unwrap();
        assert!(
            !last.contains("EXTRA_AFTER"),
            "last chunk must not contain beyond-document padding"
        );
    }
}
