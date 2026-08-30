pub mod config;
pub mod sidecar;

use crate::cloud_bridge::pairing::KeyringPairingTokenStore;
#[allow(deprecated)]
use crate::inference::config::{
    read_config, resolve_model_path, write_config, GenerationConfig, GenerationProviderKind,
};
use crate::inference::sidecar::{await_sidecar_ready, pick_port, spawn_sidecar, SidecarProcess};
use crate::privacy::{self, allows_external_generation};
use anyhow::Result;
use reqwest::blocking::Client;
use serde::Serialize;
use sha2::Digest;
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

pub enum GenerationProvider {
    Sidecar {
        process: SidecarProcess,
        model: String,
    },
    External {
        base_url: String,
        api_key: Option<String>,
        model_name: String,
    },
    Unconfigured,
}

pub struct InferenceState(pub Mutex<GenerationProvider>);

#[derive(Debug)]
enum RouteInfo {
    Unconfigured,
    Http {
        url: String,
        api_key: Option<String>,
        model: String,
    },
}

impl GenerationProvider {
    fn route_info(&self) -> RouteInfo {
        match self {
            GenerationProvider::Unconfigured => RouteInfo::Unconfigured,
            GenerationProvider::Sidecar { process, model } => RouteInfo::Http {
                url: format!("http://127.0.0.1:{}/v1/chat/completions", process.port),
                api_key: None,
                model: model.clone(),
            },
            GenerationProvider::External {
                base_url,
                api_key,
                model_name,
            } => {
                let base = base_url.trim_end_matches('/');
                let base = base.strip_suffix("/v1").unwrap_or(base);
                let model = if model_name.trim().is_empty() {
                    "default".to_string()
                } else {
                    model_name.clone()
                };
                RouteInfo::Http {
                    url: format!("{}/v1/chat/completions", base),
                    api_key: api_key.clone(),
                    model,
                }
            }
        }
    }
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
}

#[tauri::command]
pub async fn generate_text(
    system_prompt: String,
    user_prompt: String,
    state: State<'_, InferenceState>,
) -> Result<String, String> {
    let route = {
        let guard = state.0.lock().unwrap();
        guard.route_info()
    };

    match route {
        RouteInfo::Unconfigured => Err("provider-not-ready".to_string()),
        RouteInfo::Http {
            url,
            api_key,
            model,
        } => {
            let payload = ChatRequest {
                model: &model,
                messages: vec![
                    ChatMessage {
                        role: "system",
                        content: &system_prompt,
                    },
                    ChatMessage {
                        role: "user",
                        content: &user_prompt,
                    },
                ],
            };
            let client = reqwest::Client::new();
            let mut req = client.post(&url).json(&payload);
            if let Some(key) = api_key {
                req = req.header("Authorization", format!("Bearer {key}"));
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            body["choices"][0]["message"]["content"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "missing content in /v1/chat/completions response".to_string())
        }
    }
}

fn sidecar_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

fn initialize_provider_inner(
    brain_dir: &Path,
    config: &GenerationConfig,
    app: Option<&AppHandle>,
) -> Result<GenerationProvider> {
    match config.provider {
        GenerationProviderKind::Unconfigured => Ok(GenerationProvider::Unconfigured),
        GenerationProviderKind::External => {
            let base_url = config.external_url.clone().unwrap_or_default();
            if base_url.trim().is_empty() {
                return Err(anyhow::anyhow!("external URL must not be empty"));
            }
            let model_name = config
                .model_name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "default".to_string());
            Ok(GenerationProvider::External {
                base_url,
                api_key: config.api_key.clone(),
                model_name,
            })
        }
        GenerationProviderKind::Sidecar => {
            let model_rel = config
                .model_path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("sidecar requires model_path"))?;
            let binary = brain_dir.join("bin").join(sidecar_binary_name());
            if !binary.exists() {
                return Err(anyhow::anyhow!(
                    "llama-server binary not found at {}",
                    binary.display()
                ));
            }
            let model_abs = resolve_model_path(brain_dir, model_rel);
            if !model_abs.exists() {
                return Err(anyhow::anyhow!(
                    "model file not found at {}",
                    model_abs.display()
                ));
            }
            let model_name = model_rel
                .split(std::path::MAIN_SEPARATOR)
                .last()
                .unwrap_or_default()
                .to_string();
            let port = pick_port()?;
            let mut proc = spawn_sidecar(&binary, &model_abs, port)?;
            let app = app.ok_or_else(|| anyhow::anyhow!("sidecar provider requires app handle"))?;
            await_sidecar_ready(&mut proc, app)?;
            std::env::set_var("OLLAMA_BASE_URL", format!("http://127.0.0.1:{}", port));
            Ok(GenerationProvider::Sidecar {
                process: proc,
                model: model_name,
            })
        }
    }
}

pub fn initialize_provider(
    brain_dir: &Path,
    config: &GenerationConfig,
    app: &AppHandle,
) -> Result<GenerationProvider> {
    initialize_provider_inner(brain_dir, config, Some(app))
}

#[allow(deprecated)]
pub fn update_provider_with_brain_path(
    brain_path: &Path,
    config: GenerationConfig,
    state: &InferenceState,
    app: Option<&AppHandle>,
) -> Result<(), String> {
    let privacy_state = privacy::resolve_privacy_state(brain_path, &KeyringPairingTokenStore)
        .map_err(|e| e.to_string())?;
    if config.provider == GenerationProviderKind::External
        && !allows_external_generation(privacy_state.mode)
    {
        return Err("privacy-mode-strict: external generation is disabled".to_string());
    }

    let new_provider = match initialize_provider_inner(brain_path, &config, app) {
        Ok(provider) => provider,
        Err(e) => {
            let mut fallback_config = read_config(brain_path);
            fallback_config.generation = GenerationConfig::default();
            let rollback_err = write_config(brain_path, &fallback_config).err();
            if let Some(rollback_err) = rollback_err {
                return Err(format!(
                    "provider init failed: {e}; rollback failed: {rollback_err}"
                ));
            }
            let mut guard = state.0.lock().unwrap();
            *guard = GenerationProvider::Unconfigured;
            return Err(e.to_string());
        }
    };

    let mut llm_config = read_config(brain_path);
    llm_config.generation = config;

    if let Err(e) = write_config(brain_path, &llm_config) {
        let mut fallback = llm_config;
        fallback.generation = GenerationConfig::default();
        let rollback_err = write_config(brain_path, &fallback).err();
        if let Some(rollback_err) = rollback_err {
            return Err(format!(
                "settings could not be saved to disk: {e}; rollback failed: {rollback_err}"
            ));
        }
        let mut guard = state.0.lock().unwrap();
        *guard = GenerationProvider::Unconfigured;
        return Err(format!("settings could not be saved to disk: {e}"));
    }

    let mut guard = state.0.lock().unwrap();
    *guard = new_provider;
    if let Some(app) = app {
        let _ = app.emit("provider-ready", ());
    }
    Ok(())
}

#[tauri::command]
pub fn update_provider(
    config: GenerationConfig,
    state: State<'_, InferenceState>,
    app: AppHandle,
) -> Result<(), String> {
    let brain_dir = crate::get_brain_dir_inner();
    let brain_path = Path::new(&brain_dir);

    update_provider_with_brain_path(brain_path, config, &state, Some(&app))
}

#[tauri::command]
#[allow(deprecated)]
pub fn get_provider_config() -> Result<serde_json::Value, String> {
    let brain_dir = crate::get_brain_dir_inner();
    let brain_path = Path::new(&brain_dir);
    let config = read_config(brain_path);
    serde_json::to_value(&config).map_err(|e| e.to_string())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn sanitize_filename(filename: &str) -> Result<&str, String> {
    if filename.contains('/') || filename.contains('\\') {
        return Err("filename must not contain path separators".to_string());
    }
    let file_name = std::path::Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "filename must be a valid single path component".to_string())?;
    Ok(file_name)
}

fn stream_download(url: &str, dest: &Path, app: &AppHandle, event: &str) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let client = Client::new();
    let mut response = client.get(url).send()?;
    let total = response.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(dest)?;
    let mut downloaded: u64 = 0;
    let mut buf = [0u8; 65536];
    loop {
        let n = response.read(&mut buf)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n])?;
        downloaded += n as u64;
        let _ = app.emit(
            event,
            serde_json::json!({ "downloaded": downloaded, "total": total }),
        );
    }
    Ok(())
}

fn llama_server_release_url() -> Result<(String, String)> {
    if let (Ok(url), Ok(sha)) = (
        std::env::var("LLAMA_SERVER_RELEASE_URL"),
        std::env::var("LLAMA_SERVER_RELEASE_SHA256"),
    ) {
        return Ok((url, sha));
    }

    const TODO_URL: &str = "REPLACE_WITH_PINNED_LLAMA_CPP_RELEASE_URL";
    const TODO_SHA: &str = "REPLACE_WITH_SHA256_OF_BINARY";

    if TODO_URL.starts_with("REPLACE_WITH_") || TODO_SHA.starts_with("REPLACE_WITH_") {
        return Err(anyhow::anyhow!(
            "automatic sidecar download is not configured: set LLAMA_SERVER_RELEASE_URL and LLAMA_SERVER_RELEASE_SHA256 or update the hardcoded platform release values"
        ));
    }

    let url = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        TODO_URL.to_string()
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        TODO_URL.to_string()
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        TODO_URL.to_string()
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        TODO_URL.to_string()
    } else {
        return Err(anyhow::anyhow!(
            "unsupported platform for automatic llama-server download"
        ));
    };

    Ok((url, TODO_SHA.to_string()))
}

#[tauri::command]
pub fn download_sidecar_engine(app: AppHandle) -> Result<(), String> {
    let brain_dir = crate::get_brain_dir_inner();
    let brain_path = Path::new(&brain_dir);
    let (url, expected_sha) = llama_server_release_url().map_err(|e| e.to_string())?;
    let dest = brain_path.join("bin").join(sidecar_binary_name());

    stream_download(&url, &dest, &app, "sidecar-download-progress").map_err(|e| e.to_string())?;

    let actual = sha256_file(&dest).map_err(|e| e.to_string())?;
    if actual != expected_sha {
        let _ = std::fs::remove_file(&dest);
        return Err(format!(
            "checksum mismatch: expected {expected_sha}, got {actual}"
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest)
            .map_err(|e| e.to_string())?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms).map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub fn download_model_weights(
    url: String,
    filename: String,
    expected_sha256: String,
    app: AppHandle,
) -> Result<(), String> {
    let brain_dir = crate::get_brain_dir_inner();
    let brain_path = Path::new(&brain_dir);
    let filename = sanitize_filename(&filename)?;
    let dest = brain_path.join("models").join(filename);

    stream_download(&url, &dest, &app, "gguf-download-progress").map_err(|e| e.to_string())?;

    let actual = sha256_file(&dest).map_err(|e| e.to_string())?;
    if actual != expected_sha256 {
        let _ = std::fs::remove_file(&dest);
        return Err(format!(
            "model checksum mismatch: expected {expected_sha256}, got {actual}"
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn chat_request_serializes_to_openai_schema() {
        let req = ChatRequest {
            model: "llama3.2",
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: "be helpful",
                },
                ChatMessage {
                    role: "user",
                    content: "hello",
                },
            ],
        };
        let json: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(json["messages"][0]["role"], "system");
        assert_eq!(json["messages"][0]["content"], "be helpful");
        assert_eq!(json["messages"][1]["role"], "user");
        assert_eq!(json["messages"][1]["content"], "hello");
        assert!(
            json.get("stream").is_none(),
            "payload should not include stream"
        );
        assert!(
            json.get("messages").is_some(),
            "top-level 'messages' key must exist"
        );
        assert!(json["messages"].is_array(), "'messages' must be an array");
    }

    #[test]
    fn unconfigured_provider_routes_to_unconfigured() {
        let p = GenerationProvider::Unconfigured;
        assert!(matches!(p.route_info(), RouteInfo::Unconfigured));
    }

    #[test]
    fn external_provider_normalizes_url_correctly() {
        let p = GenerationProvider::External {
            base_url: "http://localhost:11434/v1".to_string(),
            api_key: None,
            model_name: "llama3.2".to_string(),
        };
        let RouteInfo::Http { url, .. } = p.route_info() else {
            panic!()
        };
        assert_eq!(url, "http://localhost:11434/v1/chat/completions");
    }

    #[test]
    fn external_provider_without_v1_suffix_gets_it_appended() {
        let p = GenerationProvider::External {
            base_url: "http://localhost:11434".to_string(),
            api_key: Some("key123".to_string()),
            model_name: "gpt-4o".to_string(),
        };
        let RouteInfo::Http {
            url,
            api_key,
            model,
        } = p.route_info()
        else {
            panic!()
        };
        assert_eq!(url, "http://localhost:11434/v1/chat/completions");
        assert_eq!(api_key.as_deref(), Some("key123"));
        assert_eq!(model, "gpt-4o");
    }

    #[test]
    fn external_provider_with_empty_model_name_defaults_to_default() {
        let p = GenerationProvider::External {
            base_url: "http://localhost:11434".to_string(),
            api_key: Some("key123".to_string()),
            model_name: "  ".to_string(),
        };
        let RouteInfo::Http { model, .. } = p.route_info() else {
            panic!()
        };
        assert_eq!(model, "default");
    }

    #[test]
    fn sha256_file_produces_correct_hash() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.bin");
        std::fs::write(&path, b"hello world").unwrap();
        let hash = sha256_file(&path).unwrap();
        assert_eq!(hash.len(), 64, "SHA-256 hex is 64 chars");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
