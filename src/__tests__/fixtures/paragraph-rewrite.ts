/** Librarian rewrite: ~80% prose churn, one new fact ($2.4M budget). */
export const PARAGRAPH_REWRITE_OLD =
  "Project Atlas is an internal tooling initiative focused on unifying deployment pipelines across engineering teams. The work began in early 2024 with a small pilot group and has since expanded to three additional squads. Stakeholders meet biweekly to review progress and adjust priorities based on platform feedback.";

export const PARAGRAPH_REWRITE_NEW =
  "Atlas is the company's cross-team deployment unification program, launched as a pilot in Q1 2024 and now covering four engineering squads. Executive sponsors allocated a dedicated budget of $2.4 million for FY2025 infrastructure upgrades tied to the rollout. Platform leads hold biweekly syncs to triage blockers surfaced from downstream consumers.";

/** The single new fact the librarian introduced. */
export const PARAGRAPH_REWRITE_NEW_FACT = "$2.4 million";

/** Moderate rewrite: enough churn for word diff, below side-by-side threshold. */
export const MODERATE_REWRITE_OLD = PARAGRAPH_REWRITE_OLD;

export const MODERATE_REWRITE_NEW =
  "Project Atlas is an internal tooling initiative focused on unifying deployment pipelines across engineering teams. The work began in early 2024 with a small pilot group and has since expanded to three additional squads. Executive sponsors allocated $2.4 million in dedicated funding for FY2025. Stakeholders meet biweekly to review progress and adjust priorities based on platform feedback.";

/** Completely unrelated text — always triggers side-by-side. */
export const HIGH_CHURN_OLD =
  "The quarterly revenue report shows steady growth across all regions.";

export const HIGH_CHURN_NEW =
  "Neptune's atmosphere contains trace amounts of methane discovered during the 1989 Voyager flyby.";
