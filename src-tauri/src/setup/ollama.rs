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

pub fn parse_models_response(json: &str) -> Result<Vec<String>> {
    let resp: OllamaListResponse = serde_json::from_str(json)?;
    Ok(resp.models.into_iter().map(|m| m.name).collect())
}

pub fn check_ollama() -> OllamaStatus {
    let installed = Command::new("ollama").arg("--version").output().is_ok();

    if !installed {
        return OllamaStatus { installed: false, running: false, models: vec![] };
    }

    match reqwest::blocking::get("http://localhost:11434/api/tags") {
        Ok(resp) if resp.status().is_success() => {
            let text = resp.text().unwrap_or_default();
            let models = parse_models_response(&text).unwrap_or_default();
            OllamaStatus { installed: true, running: true, models }
        }
        _ => OllamaStatus { installed: true, running: false, models: vec![] },
    }
}

pub fn list_local_models() -> Result<Vec<String>> {
    let text = reqwest::blocking::get("http://localhost:11434/api/tags")?.text()?;
    parse_models_response(&text)
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
}
