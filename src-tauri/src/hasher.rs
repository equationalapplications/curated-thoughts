use sha2::{Digest, Sha256};

pub fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_is_deterministic() {
        assert_eq!(hash_bytes(b"hello"), hash_bytes(b"hello"));
    }

    #[test]
    fn test_different_inputs_produce_different_hashes() {
        assert_ne!(hash_bytes(b"hello"), hash_bytes(b"world"));
    }

    #[test]
    fn test_hash_is_64_hex_chars() {
        assert_eq!(hash_bytes(b"test").len(), 64);
    }
}
