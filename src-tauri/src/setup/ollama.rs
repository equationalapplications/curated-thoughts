use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OllamaStatus {
    pub installed: bool,
    pub running: bool,
    pub models: Vec<String>,
}

#[derive(Deserialize)]
struct OllamaListResponse {
    models: Vec<OllamaModel>,
}

#[derive(Deserialize)]
struct OllamaModel {
    name: String,
}

fn ram_gb() -> u64 {
    // macOS reports byte-accurate RAM via sysctl. Linux has no hw.memsize
    // sysctl MIB (the command fails there, which silently pinned everyone to
    // the 8 GB bucket) — parse /proc/meminfo instead, then fall back to the
    // historical 8 GB bucket.
    Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|bytes| bytes / (1024 * 1024 * 1024))
        .or_else(meminfo_ram_gb)
        .unwrap_or(8)
}

/// Total RAM in GB parsed from `/proc/meminfo` (Linux), if readable.
fn meminfo_ram_gb() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_meminfo_ram_gb(&info)
}

fn parse_meminfo_ram_gb(info: &str) -> Option<u64> {
    let kb: u64 = info
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    (kb > 0).then_some(kb / (1024 * 1024))
}

pub fn recommended_model() -> &'static str {
    match ram_gb() {
        0..=7 => "qwen2.5:0.5b",
        8..=15 => "llama3.2:1b",
        _ => "llama3.2:3b",
    }
}

pub fn parse_models_response(json: &str) -> Result<Vec<String>> {
    let resp: OllamaListResponse = serde_json::from_str(json)?;
    Ok(resp.models.into_iter().map(|m| m.name).collect())
}

// GUI apps on macOS don't inherit the user's shell PATH, so Homebrew
// binaries at /opt/homebrew/bin are invisible to Command::new("ollama").
fn find_ollama() -> Option<std::path::PathBuf> {
    let known = [
        "/opt/homebrew/bin/ollama", // Apple Silicon Homebrew
        "/usr/local/bin/ollama",    // Intel Homebrew / manual install
        "/Applications/Ollama.app/Contents/Resources/ollama",
    ];
    // Try shell PATH first (works in terminal-launched dev builds)
    if let Ok(out) = Command::new("sh").args(["-c", "which ollama"]).output() {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                return Some(std::path::PathBuf::from(p));
            }
        }
    }
    known
        .iter()
        .map(std::path::Path::new)
        .find(|p| p.exists())
        .map(|p| p.to_path_buf())
}

pub fn check_ollama() -> OllamaStatus {
    let Some(bin) = find_ollama() else {
        return OllamaStatus {
            installed: false,
            running: false,
            models: vec![],
        };
    };

    match reqwest::blocking::get("http://localhost:11434/api/tags") {
        Ok(resp) if resp.status().is_success() => {
            let text = resp.text().unwrap_or_default();
            let models = parse_models_response(&text).unwrap_or_default();
            OllamaStatus {
                installed: true,
                running: true,
                models,
            }
        }
        _ => {
            let _ = bin; // binary found but server not up yet
            OllamaStatus {
                installed: true,
                running: false,
                models: vec![],
            }
        }
    }
}

pub fn list_local_models() -> Result<Vec<String>> {
    let text = reqwest::blocking::get("http://localhost:11434/api/tags")?.text()?;
    parse_models_response(&text)
}

pub fn start_ollama_server() -> Result<()> {
    let bin = find_ollama().ok_or_else(|| anyhow::anyhow!("ollama not found"))?;
    Command::new(bin)
        .arg("serve")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    // Give the server a moment to bind the port
    std::thread::sleep(std::time::Duration::from_secs(2));
    Ok(())
}

pub fn pull_model<F>(model_id: &str, on_progress: F) -> Result<()>
where
    F: Fn(u64, u64),
{
    use std::io::{BufRead, BufReader};

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post("http://localhost:11434/api/pull")
        .json(&serde_json::json!({ "name": model_id }))
        .send()?;

    let reader = BufReader::new(resp);
    for line in reader.lines() {
        let line = line?;
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
            let completed = val["completed"].as_u64().unwrap_or(0);
            let total = val["total"].as_u64().unwrap_or(1);
            on_progress(completed, total);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_models_response_extracts_names() {
        let json = r#"{"models":[{"name":"llama3.2:3b"},{"name":"phi4-mini:latest"}]}"#;
        let models = parse_models_response(json).unwrap();
        assert_eq!(models, vec!["llama3.2:3b", "phi4-mini:latest"]);
    }

    #[test]
    fn test_parse_models_response_empty() {
        let json = r#"{"models":[]}"#;
        let models = parse_models_response(json).unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn test_parse_models_response_invalid_json_errors() {
        assert!(parse_models_response("not json").is_err());
    }

    #[test]
    fn test_parse_meminfo_ram_gb() {
        // 16 GB machine
        assert_eq!(
            parse_meminfo_ram_gb("MemTotal:       16384000 kB\n"),
            Some(15)
        );
        // 8 GB machine
        assert_eq!(
            parse_meminfo_ram_gb("MemTotal:        8388608 kB\n"),
            Some(8)
        );
        // Tiny / unparseable inputs
        assert_eq!(parse_meminfo_ram_gb("MemTotal:       1024 kB\n"), Some(0));
        assert_eq!(parse_meminfo_ram_gb("no memory line here\n"), None);
    }
}
