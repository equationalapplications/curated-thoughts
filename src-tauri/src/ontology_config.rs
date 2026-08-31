//! The user's ontology selection, persisted in config.json.
//!
//! Rust owns the *selection* only. The manifest itself never crosses into
//! Rust — the TypeScript engine imports it from an npm schema package and
//! seeds it via `createWiki`. See the D2 decision in
//! docs/superpowers/specs/2026-08-28-strict-schema-ea-adoption-spec.md.

use serde::{Deserialize, Serialize};

/// Which ontology the user picked. Absent (`None` on the block) means the
/// question has never been answered, and the surface default applies:
/// `SchemaOrg` on Desktop, `SchemaSoftwareOrg` in the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OntologySelection {
    /// `@equationalapplications/schema-org-llm-wiki` — general purpose.
    SchemaOrg,
    /// `@equationalapplications/schema-software-org` — software organizations.
    SchemaSoftwareOrg,
    /// No fixed manifest; the engine may propose new types.
    Emergent,
    /// No typed graph at all.
    Off,
}

impl OntologySelection {
    /// The Desktop setup wizard's default.
    pub const DESKTOP_DEFAULT: OntologySelection = OntologySelection::SchemaOrg;
    /// The `--onboard` CLI default.
    pub const CLI_DEFAULT: OntologySelection = OntologySelection::SchemaSoftwareOrg;
}

/// The `ontology` block of config.json.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OntologyConfigBlock {
    /// `None` = never chosen. Unparseable values load as `None` (lenient).
    #[serde(default)]
    pub schema: Option<OntologySelection>,
}
