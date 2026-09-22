//! Intake for core-llm-wiki `onDiagnostic` reports (spec CT-REQ-DIAG-01).
//!
//! Diagnostics carry identifiers, counts and slugs only — never fact text or
//! LLM output (upstream contract) — so they are safe to log verbatim.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiDiagnostic {
    pub code: String,
    pub severity: String,
    pub operation: String,
    pub trigger: String,
    pub entity_id: String,
    pub at: f64,
    pub message: String,
    #[serde(default)]
    pub detail: Option<serde_json::Value>,
}

/// In-memory per-severity counts since launch (or the last vault switch).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiagnosticCounts {
    pub errors: u32,
    pub warnings: u32,
}

impl DiagnosticCounts {
    pub fn record(&mut self, severity: &str) {
        match severity {
            "error" => self.errors = self.errors.saturating_add(1),
            "warn" => self.warnings = self.warnings.saturating_add(1),
            _ => {}
        }
    }
}

pub fn log_line(d: &WikiDiagnostic) -> String {
    let detail = d
        .detail
        .as_ref()
        .map(|v| format!(" detail={v}"))
        .unwrap_or_default();
    format!(
        "[wiki-diagnostic] {} {} op={} trigger={} entity={} msg={}{}",
        d.severity, d.code, d.operation, d.trigger, d.entity_id, d.message, detail
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(severity: &str) -> WikiDiagnostic {
        serde_json::from_value(serde_json::json!({
            "code": "embedding_failed",
            "severity": severity,
            "operation": "ontologyBackfill",
            "trigger": "call",
            "entityId": "tier_fact",
            "at": 1_758_000_000_000_i64,
            "message": "Embedding failed.",
            "detail": { "itemIndex": 3 }
        }))
        .unwrap()
    }

    #[test]
    fn deserializes_engine_camel_case_shape() {
        let d = sample("error");
        assert_eq!(d.entity_id, "tier_fact");
        assert_eq!(d.detail.unwrap()["itemIndex"], 3);
    }

    #[test]
    fn counts_errors_and_warnings_ignores_info() {
        let mut c = DiagnosticCounts::default();
        c.record("error");
        c.record("warn");
        c.record("warn");
        c.record("info");
        assert_eq!(c, DiagnosticCounts { errors: 1, warnings: 2 });
    }

    #[test]
    fn log_line_includes_code_entity_and_detail() {
        let line = log_line(&sample("warn"));
        assert!(line.contains("warn embedding_failed"));
        assert!(line.contains("entity=tier_fact"));
        assert!(line.contains("detail={\"itemIndex\":3}"));
    }
}
