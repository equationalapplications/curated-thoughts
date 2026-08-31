<!--
Generation command (re-run from repo root after component changes, then commit):
  git ls-tree -r --name-only HEAD -- src | grep '\.tsx$' | grep -v __tests__ | sort
Scan per file: aria-[a-z-]+ attributes; role="status|alert" or aria-live => live region;
  role="dialog" or aria-modal => dialog. Status derivation:
  REMEDIATED-PR127 = files touched by d4fe4a6 (jsx-a11y remediation, 8 files);
  PASS = a11y primitives (unit-tested) + FactPowerMenu (axe smoke test);
  WATCH = live region or dialog/modal present -> manual screen-reader check required;
  TODO-PHASEn = per spec phasing table (2=setup, 3=review, 4=brain+palette/peek/editor,
  5=settings+privacy, 6=timeline/tasks/library/health/shell remainder).
-->

# Conformance Ledger — WCAG 2.2 AA Foundation (Phase 1)

Generated: 2026-08-31 · branch `feat/a11y-wcag-aa-foundation` @ 8f9cb2e

**What this ledger is:** a *presence audit*, not a conformance claim. It records which 
accessibility attributes each component currently exhibits and which remediation phase 
owns it. Conformance is established by the CI gates (jsx-a11y lint, axe smoke test, 
contrast tests) plus `docs/a11y/manual-checklist.md`.

**Component counts:** HEAD contains **115** `.tsx` files under `src/` — **68** components 
(this ledger, `__tests__` excluded) + **47** colocated test files. The original "107 components" 
figure is the `.tsx` count at branch base a739cfb (65 non-test + 42 tests); this branch added 
`src/a11y/{Announcer,SkipLink,VisuallyHidden}.tsx` and 5 test files.

## Status legend

| Status | Meaning |
|---|---|
| PASS | Automated coverage exists (primitive unit tests / axe smoke) |
| REMEDIATED-PR127 | jsx-a11y violations remediated in d4fe4a6 (Task 1) |
| WATCH | Uses live region and/or dialog/modal — needs manual SR verification |
| TODO-PHASE2..6 | Remediation assigned to that phase per the spec phasing table |

## Ledger (68 components)

| Component | aria-* present | Live region | Dialog/modal | Status | Notes |
|---|---|---|---|---|---|
| `src/App.tsx` | — | — | — | TODO-PHASE1 | — |
| `src/a11y/Announcer.tsx` | aria-atomic, aria-live | yes | — | PASS | phase-1 primitive; unit-tested (5e17f57) |
| `src/a11y/SkipLink.tsx` | — | — | — | PASS | phase-1 primitive; unit-tested (5e17f57) |
| `src/a11y/VisuallyHidden.tsx` | — | — | — | PASS | phase-1 primitive; unit-tested (5e17f57) |
| `src/components/brain/ConnectionsPanel.tsx` | aria-label | — | — | TODO-PHASE4 | — |
| `src/components/brain/EntityList.tsx` | aria-label | — | — | REMEDIATED-PR127 | jsx-a11y remediation (d4fe4a6) |
| `src/components/brain/EntityPage.tsx` | — | yes | — | WATCH | live region — manual SR check |
| `src/components/brain/EntitySummarySection.tsx` | — | yes | — | WATCH | live region — manual SR check |
| `src/components/brain/EntityWikilinkSuggestion.tsx` | aria-label, aria-selected | — | — | REMEDIATED-PR127 | jsx-a11y remediation (d4fe4a6) |
| `src/components/brain/FactCard.tsx` | aria-expanded, aria-label | yes | — | WATCH | live region — manual SR check |
| `src/components/brain/FactPowerMenu.tsx` | aria-label | — | yes | PASS | axe smoke test: zero violations |
| `src/components/brain/WikilinkText.tsx` | — | — | — | TODO-PHASE4 | — |
| `src/components/health/ProviderNotice.tsx` | aria-hidden, aria-live | yes | — | WATCH | live region — manual SR check |
| `src/components/modes/BrainMode.tsx` | aria-label, aria-live | yes | — | WATCH | live region — manual SR check |
| `src/components/modes/LibraryMode.tsx` | aria-label | — | — | TODO-PHASE6 | — |
| `src/components/modes/ReviewMode.tsx` | — | — | — | REMEDIATED-PR127 | jsx-a11y remediation (d4fe4a6) |
| `src/components/modes/TasksMode.tsx` | aria-label | yes | — | REMEDIATED-PR127 | jsx-a11y remediation (d4fe4a6) |
| `src/components/modes/TimelineMode.tsx` | — | yes | — | WATCH | live region — manual SR check |
| `src/components/privacy/EphemeralDisclosureModal.tsx` | aria-labelledby, aria-modal | — | yes | WATCH | dialog/modal — manual SR check |
| `src/components/privacy/MigrationDisclosureModal.tsx` | aria-labelledby, aria-modal | — | yes | WATCH | dialog/modal — manual SR check |
| `src/components/privacy/PrivacyModeCards.tsx` | aria-label | — | — | TODO-PHASE5 | — |
| `src/components/privacy/PrivacyShieldIcon.tsx` | aria-hidden | — | — | TODO-PHASE5 | — |
| `src/components/review/MemoryChunk.tsx` | — | — | — | TODO-PHASE3 | — |
| `src/components/review/PendingLinksPanel.tsx` | — | yes | — | WATCH | live region — manual SR check |
| `src/components/review/ProposalDiff.tsx` | — | — | — | TODO-PHASE3 | — |
| `src/components/review/ProposalItemRow.tsx` | aria-label, aria-pressed | — | — | TODO-PHASE3 | — |
| `src/components/review/ReviewEvidencePanel.tsx` | aria-label | — | — | TODO-PHASE3 | — |
| `src/components/review/ReviewProposalEditor.tsx` | — | — | — | TODO-PHASE3 | — |
| `src/components/review/ReviewQueueList.tsx` | aria-label, aria-pressed | — | — | TODO-PHASE3 | — |
| `src/components/settings/AgentIntegrationPanel.tsx` | aria-live | yes | — | WATCH | live region — manual SR check |
| `src/components/settings/AppearancePanel.tsx` | aria-label | — | — | TODO-PHASE5 | — |
| `src/components/settings/CloudBridgePanel.tsx` | aria-label, aria-live | yes | — | WATCH | live region — manual SR check |
| `src/components/settings/EmbeddingPanel.tsx` | — | — | — | TODO-PHASE5 | — |
| `src/components/settings/FolderRulesPanel.tsx` | — | — | — | TODO-PHASE5 | — |
| `src/components/settings/GenerationPanel.tsx` | — | — | — | TODO-PHASE5 | — |
| `src/components/settings/MaintenanceDashboard.tsx` | aria-live | yes | — | WATCH | live region — manual SR check |
| `src/components/settings/ModelPanel.tsx` | — | — | — | TODO-PHASE5 | — |
| `src/components/settings/OntologyPanel.tsx` | — | yes | — | WATCH | live region — manual SR check |
| `src/components/settings/PrivacyPanel.tsx` | — | — | — | TODO-PHASE5 | — |
| `src/components/settings/SettingsScreen.tsx` | aria-label, aria-selected | — | — | REMEDIATED-PR127 | jsx-a11y remediation (d4fe4a6) |
| `src/components/settings/VaultPanel.tsx` | — | — | — | TODO-PHASE5 | — |
| `src/components/setup/OntologyChoice.tsx` | — | yes | — | WATCH | live region — manual SR check |
| `src/components/setup/SetupWizard.tsx` | — | — | — | TODO-PHASE2 | — |
| `src/components/setup/StepDone.tsx` | — | — | — | TODO-PHASE2 | — |
| `src/components/setup/StepFastembed.tsx` | — | — | — | TODO-PHASE2 | — |
| `src/components/setup/StepIndicator.tsx` | aria-current, aria-label, aria-valuemax, aria-valuemin, aria-valuenow | — | — | TODO-PHASE2 | — |
| `src/components/setup/StepModel.tsx` | — | — | — | TODO-PHASE2 | — |
| `src/components/setup/StepOllama.tsx` | — | — | — | REMEDIATED-PR127 | jsx-a11y remediation (d4fe4a6) |
| `src/components/setup/StepPrivacy.tsx` | — | — | — | TODO-PHASE2 | — |
| `src/components/setup/StepVaultPicker.tsx` | — | — | — | TODO-PHASE2 | — |
| `src/components/setup/StepWatchItThink.tsx` | aria-label, aria-live | yes | — | WATCH | live region — manual SR check |
| `src/components/setup/StepWelcome.tsx` | — | — | — | TODO-PHASE2 | — |
| `src/components/setup/WizardStep.tsx` | aria-busy, aria-hidden, aria-labelledby | — | — | REMEDIATED-PR127 | jsx-a11y remediation (d4fe4a6) |
| `src/components/shell/ActivityFeedPanel.tsx` | aria-label, aria-modal | yes | yes | WATCH | live region + dialog/modal — manual SR check |
| `src/components/shell/AppShell.tsx` | — | — | — | TODO-PHASE6 | — |
| `src/components/shell/CommandPalette.tsx` | aria-activedescendant, aria-controls, aria-expanded, aria-label, aria-modal, aria-selected | — | yes | REMEDIATED-PR127 | jsx-a11y remediation (d4fe4a6) |
| `src/components/shell/EditorPane.tsx` | aria-hidden | yes | — | WATCH | live region — manual SR check |
| `src/components/shell/FolderTree.tsx` | aria-label | — | — | TODO-PHASE6 | — |
| `src/components/shell/ModeRail.tsx` | aria-current, aria-hidden, aria-label | — | — | TODO-PHASE6 | — |
| `src/components/shell/OkfInteropBar.tsx` | aria-label | — | yes | WATCH | dialog/modal — manual SR check |
| `src/components/shell/PeekPanel.tsx` | aria-label, aria-modal | yes | yes | WATCH | live region + dialog/modal — manual SR check |
| `src/components/shell/RelatedNotes.tsx` | aria-label | — | — | TODO-PHASE6 | — |
| `src/components/shell/SearchResults.tsx` | aria-label | — | — | TODO-PHASE6 | — |
| `src/components/shell/SplashScreen.tsx` | aria-valuemax, aria-valuenow | yes | — | WATCH | live region — manual SR check |
| `src/components/shell/StatusBar.tsx` | aria-label | — | — | TODO-PHASE6 | — |
| `src/components/timeline/TimelineFeed.tsx` | — | — | — | TODO-PHASE6 | — |
| `src/lib/ThemeContext.tsx` | — | — | — | TODO-PHASE1 | — |
| `src/main.tsx` | — | — | — | TODO-PHASE1 | — |

## Cross-cutting gaps (phase 1 scope, tracked separately)

- `src/components/shell/AppShell.tsx` does not yet mount `SkipLink` / `AnnouncerProvider` — Task 4 wiring.
- Focus-trap (`trapped`) applies to PeekPanel/EditorPane/palette dialogs from phase 4 onward.