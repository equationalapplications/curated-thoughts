use std::time::Duration;

use super::heartbeat::Stage;
use crate::embedder::{EmbedProfile, EXTERNAL_EMBED_TIMEOUT_SECS};
use crate::embedder::OLLAMA_TIMEOUT_SECS;

/// Slack above a stage's own ceiling before the watchdog calls it stalled.
const SLACK_SECS: u64 = 60;

/// The HTTP ceiling of the *active* embed profile. Deriving one fixed budget
/// from the external profile's 120s would false-trip every Ollama embed at
/// roughly three minutes (spec §2.2).
pub fn embed_ceiling_secs(profile: &EmbedProfile) -> u64 {
    match profile {
        EmbedProfile::Local { .. } => OLLAMA_TIMEOUT_SECS,
        EmbedProfile::Cloud { .. } | EmbedProfile::External { .. } => {
            EXTERNAL_EMBED_TIMEOUT_SECS
        }
    }
}

/// Budget for a stage. `None` means the stage never trips.
pub fn budget_for(
    stage: Stage,
    profile: &EmbedProfile,
    gen_timeout_secs: u64,
) -> Option<Duration> {
    let secs = match stage {
        // Blocked on `recv()` with an empty channel is correct behavior.
        Stage::Idle => return None,
        Stage::Reading => 60,
        // `pdf_extract` / docx have no internal ceiling.
        Stage::Extracting => 300,
        Stage::Chunking => 120,
        Stage::Embedding => embed_ceiling_secs(profile) + SLACK_SECS,
        Stage::Summarizing => gen_timeout_secs + SLACK_SECS,
        // Per entity, not per flush batch (spec §2.2).
        Stage::Linking => 60,
        Stage::Committing => 60,
        // Unindexed `LIKE` scan over wiki_pages (pipeline/mod.rs:229-256).
        Stage::Deleting => 120,
    };
    Some(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::EmbedProfile;

    fn ollama() -> EmbedProfile {
        EmbedProfile::Local {
            model: "nomic-embed-text".to_string(),
        }
    }

    #[test]
    fn idle_never_trips() {
        assert!(budget_for(Stage::Idle, &ollama(), 600).is_none());
    }

    #[test]
    fn embedding_budget_follows_the_active_profile() {
        // Ollama's ceiling is 600s; a budget computed from the external
        // profile's 120s would false-trip every local embed at ~3 minutes.
        let local = budget_for(Stage::Embedding, &ollama(), 600).unwrap();
        assert_eq!(local.as_secs(), 600 + 60);

        let cloud = EmbedProfile::Cloud {
            provider: crate::embedder::CloudProvider::OpenAi,
            model: "text-embedding-3-small".to_string(),
            api_key: String::new(),
        };
        let remote = budget_for(Stage::Embedding, &cloud, 600).unwrap();
        assert_eq!(remote.as_secs(), 120 + 60);
    }

    #[test]
    fn summarizing_budget_follows_configured_generation_timeout() {
        let b = budget_for(Stage::Summarizing, &ollama(), 900).unwrap();
        assert_eq!(b.as_secs(), 900 + 60);
    }

    #[test]
    fn every_non_idle_stage_has_a_budget() {
        for stage in [
            Stage::Reading,
            Stage::Extracting,
            Stage::Chunking,
            Stage::Embedding,
            Stage::Summarizing,
            Stage::Linking,
            Stage::Committing,
            Stage::Deleting,
        ] {
            assert!(
                budget_for(stage, &ollama(), 600).is_some(),
                "stage {:?} has no budget",
                stage
            );
        }
    }
}
