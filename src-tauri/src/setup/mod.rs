pub mod ollama;
pub use ollama::{check_ollama, list_local_models, pull_model, start_ollama_server, OllamaStatus};
