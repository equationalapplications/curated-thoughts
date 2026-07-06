use std::collections::HashSet;

const OKF_RESERVED_CONCEPT_NAMES: &[&str] = &["index", "log"];

const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

fn short_hash(value: &str) -> String {
    let mut h1: u32 = 5381;
    let mut h2: u32 = 52711;
    for ch in value.chars() {
        let c = ch as u32;
        h1 = h1.wrapping_mul(33) ^ c;
        h2 = h2.wrapping_mul(31) ^ c;
    }
    format!("{:08x}{:08x}", h1, h2)
}

fn is_windows_reserved_name(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or(name).to_ascii_lowercase();
    WINDOWS_RESERVED_NAMES.contains(&base.as_str())
}

/// Sanitize an entity id for use as a directory name (profile §2).
pub fn sanitize_for_filename(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_start_matches('.')
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_");

    const MAX_BASE: usize = 200;
    let trimmed = if sanitized.len() > MAX_BASE {
        sanitized[..MAX_BASE].to_string()
    } else {
        sanitized
    };

    let mut base_name = if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "entity".to_string()
    } else {
        trimmed
    };

    let without_trailing: String = base_name.trim_end_matches(['.', ' ']).to_string();
    let had_trailing_dot_space = without_trailing != base_name;
    if had_trailing_dot_space {
        base_name = if without_trailing.is_empty() {
            "entity".to_string()
        } else {
            without_trailing
        };
    }

    let windows_reserved = is_windows_reserved_name(&base_name);
    let needs_suffix = base_name != value
        || sanitized.len() > MAX_BASE
        || had_trailing_dot_space
        || windows_reserved;

    if !needs_suffix {
        return base_name;
    }

    format!("{}-{}", base_name, short_hash(value))
}

/// Sanitize a fact/task id for use as a concept filename (profile §2).
pub fn sanitize_concept_id(id: &str) -> String {
    let sanitized = sanitize_for_filename(id);
    let reserved: HashSet<&str> = OKF_RESERVED_CONCEPT_NAMES.iter().copied().collect();
    if reserved.contains(sanitized.to_ascii_lowercase().as_str()) {
        format!("{}-{}", sanitized, short_hash(id))
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_safe_ids() {
        assert_eq!(sanitize_for_filename("alice"), "alice");
        assert_eq!(sanitize_concept_id("fact_aaa"), "fact_aaa");
    }

    #[test]
    fn hashes_reserved_concept_names() {
        assert!(sanitize_concept_id("index").contains('-'));
        assert!(sanitize_concept_id("log").contains('-'));
    }
}
