use super::PrivacyMode;

pub fn allows_cloud_bridge(mode: PrivacyMode) -> bool {
    matches!(mode, PrivacyMode::Connected)
}

pub fn allows_external_generation(mode: PrivacyMode) -> bool {
    matches!(mode, PrivacyMode::Ephemeral | PrivacyMode::Connected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_allows_neither_bridge_nor_external() {
        assert!(!allows_cloud_bridge(PrivacyMode::Strict));
        assert!(!allows_external_generation(PrivacyMode::Strict));
    }

    #[test]
    fn ephemeral_allows_external_not_bridge() {
        assert!(!allows_cloud_bridge(PrivacyMode::Ephemeral));
        assert!(allows_external_generation(PrivacyMode::Ephemeral));
    }

    #[test]
    fn connected_allows_both() {
        assert!(allows_cloud_bridge(PrivacyMode::Connected));
        assert!(allows_external_generation(PrivacyMode::Connected));
    }
}
