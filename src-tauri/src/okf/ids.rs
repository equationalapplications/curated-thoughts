use rand::Rng;

/// Generate a random ID with an optional prefix (24 hex chars, matching core-llm-wiki).
pub fn generate_id(prefix: &str) -> String {
    let mut bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut bytes);
    format!("{prefix}{}", hex::encode(bytes))
}
