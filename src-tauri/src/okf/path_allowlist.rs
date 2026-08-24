/// Profile §1 / §8: only bundle layout paths may be parsed as concepts.
pub fn is_allowed_okf_path(file_path: &str) -> bool {
    let normalized = file_path.trim_start_matches("./").replace('\\', "/");
    if normalized.split('/').any(|seg| seg == "." || seg == "..") {
        return false;
    }

    if normalized == "index.md" {
        return true;
    }

    let Some(rest) = normalized.strip_prefix("entities/") else {
        return false;
    };
    let Some((entity_dir, tail)) = rest.split_once('/') else {
        return false;
    };
    if entity_dir.is_empty() {
        return false;
    }

    match tail {
        "index.md" | "log.md" => true,
        path if path.starts_with("facts/") => {
            let name = path.strip_prefix("facts/").unwrap_or("");
            !name.is_empty() && name.ends_with(".md") && !name.contains('/')
        }
        path if path.starts_with("tasks/") => {
            let name = path.strip_prefix("tasks/").unwrap_or("");
            !name.is_empty() && name.ends_with(".md") && !name.contains('/')
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_layout_paths() {
        assert!(is_allowed_okf_path("index.md"));
        assert!(is_allowed_okf_path("entities/demo/index.md"));
        assert!(is_allowed_okf_path("entities/demo/facts/fact_a.md"));
    }

    #[test]
    fn rejects_readme_trap() {
        assert!(!is_allowed_okf_path("README.md"));
    }
}
