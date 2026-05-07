//! Shared size limits for non-prose chunkers (approximate embedder budget alignment).

/// Rough character budget per chunk (~100 word prose equivalent + margin).
pub(crate) fn target_chars() -> usize {
    1600
}

pub(crate) fn overlap_chars() -> usize {
    120
}

pub(crate) fn code_overlap_lines() -> usize {
    2
}
