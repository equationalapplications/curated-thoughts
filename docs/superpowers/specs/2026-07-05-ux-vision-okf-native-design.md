# Spec: UX Vision — OKF-Native Curated Thoughts

**Date:** 2026-07-05
**Status:** Phase 1 — Shell (implemented); Phase 2 — Review editorial desk (Slices 1, 3–4 implemented; `edited_payload` editing + library deep-links remain); Phase 3 backend dependency (implemented, v1.10.0); Phase 4 — Brain entity pages + cross-mode routing (implemented 2026-07-06, see plan 2026-07-06-phase-4-brain-entity-pages.md)
**Type:** North-star UX vision. Each phase below gets its own implementation plan; the backend OKF-native data-model migration gets its own separate spec (`2026-07-05-okf-backend-migration-design.md` — implemented).
**Related:** `../../../../clanker/docs/superpowers/specs/2026-07-04-okf-import-support-design.md` (OKF bundle format and import semantics), `2026-05-05-second-brain-app-design.md` (original app design), `../../../../expo-llm-wiki/docs/okf-profile.md` (normative llm-wiki OKF profile v1 — postdates this spec; binds phase 6, see the backend spec's addendum)

## Problem

The UI was built feature-by-feature as the backend grew, and it shows: the review queue — the heart of the human-in-the-loop design — is a modal with a plain-text preview and approve/reject buttons only (no editing, no diff, no source evidence, despite the README promising "approve or edit"). Settings is seven panels stacked in one scrolling modal. Wiki pages have no clickable wikilinks, no backlinks, and no graph navigation even though the backend has graph adapters. Librarian activity is invisible beyond a small indexing widget. Errors are swallowed in empty `catch` blocks. Total styling is ~116 lines of CSS.

Meanwhile, the OKF format (entities, facts, tasks, edges, event log) has become the shared memory model across Curated Thoughts and Clanker, and the product direction is to make OKF patterns first-class rather than an export afterthought.

## Decisions Made During Brainstorming

- **Core activity:** curating the wiki. Human-in-the-loop curation is the heart of the app; reading source documents is secondary.
- **Review experience:** full editorial desk — a first-class screen with inline editing, suggestion-style diffs, and side-by-side source evidence.
- **Wiki navigation:** clickable wikilinks, backlinks panel, browser-style history. Graph visualization deferred.
- **Librarian visibility:** persistent status bar plus an openable activity feed. Trust through transparency.
- **OKF depth:** native data model. The vault brain *is* an OKF graph; entities, facts, tasks, edges, and the event log become UI concepts, not just export shapes.
- **OKF surface area:** full — entity/fact pages, a Tasks mode, and a Timeline mode all in the vision.
- **Audience:** layered. Friendly PKM framing by default ("pages, links, sources"), power-user detail (raw ids, provenance, embedding info) one level deeper.
- **Scope of this spec:** UX vision only. Backend schema and librarian-output changes are a separate spec (Phase 3 dependency below).
- **Shell concept:** Approach A (activity-rail workspace), blended with inbox mechanics in Review and suggestion-diff editing from the "everything is a page" concept.
- **Privacy:** explicit three-mode privacy posture (Strict / Ephemeral cloud inference / Connected agent — renamed from "Full cloud sync", see §6), enforced by the UI, ambient in the status bar, chosen during onboarding.

## 1. Shell & Navigation

**Layout:** thin fixed left icon rail → contextual sidebar (per mode) → main content → optional right panel (per mode). Slim persistent status bar at the bottom.

**Rail items (top to bottom):** Brain, Review (badge shows queue count), Library, Timeline, Tasks, then a spacer, Activity (pulse icon), Settings. `⌘1`–`⌘5` switch modes.

**Search is not a mode.** Global `⌘K` command palette plus a search field in each mode's sidebar.

**Cross-links everywhere.** Any reference to an entity, fact, document, or proposal — in any mode — is clickable and jumps to the owning mode with the target focused. Browser-style back/forward history buttons in the header. This is the antidote to mode-switch context loss.

**Peek views.** A plain click navigates; `Option`+click (or a peek affordance on hover) opens the target in a temporary slide-over panel instead, so a user mid-edit can check a source document or entity without leaving their current mode. Peek panels are read-only and dismiss on `Esc` or click-outside; "Open in [mode]" inside the peek promotes it to full navigation. This matters most in Brain and Review, where following a source link into Library would otherwise destroy editorial flow.

**Status bar:** left = librarian state ("Idle", "Embedding 3 documents…", "Synthesizing…"); center = generation-model and embedder health dots plus the privacy-mode shield glyph; right = vault name and switcher. Clicking any segment opens the Activity feed panel (privacy glyph opens Settings → Privacy).

**Window title:** current mode + focused item name.

The existing `AppShell` three-pane layout becomes Brain mode's layout. The setup wizard and modals are reworked per sections below.

## 2. Brain Mode (OKF-native knowledge)

**Sidebar:** an entity list, not a folder tree. Grouped by OKF entity type (Person, Project, Concept, …), sortable by recently-updated or most-connected. Filter box at top. A "+ New entity" button — manual curation is allowed, not just librarian proposals.

**Main content — the entity page.** A composed view, not a raw markdown file:

- **Header:** entity name, type chip, created/updated dates, fact count.
- **Summary block:** the entity's `index.md` prose, editable rich text (BlockNote stays).
- **Facts list:** each fact is a card row — text, confidence/source chip, updated date. Inline edit, add, archive. Clicking a source chip jumps to the Library document at the exact chunk.
- **Tasks strip:** open tasks referencing this entity (links into Tasks mode).
- **Timeline strip:** recent events for this entity (links into Timeline mode).

**Right panel — Connections:** backlinks (entities whose facts/edges point here), outgoing edges grouped by edge type, then related-by-similarity results (the current RelatedNotes feature survives here, demoted below structural links). Every item clickable.

**Wikilinks:** `[[Entity]]` in summary and fact text renders as a chip; click to navigate. Autocomplete triggers on `[[` while editing.

**Layering:** the default UI says "linked from", not "inbound edges". A "…" menu per fact/edge exposes the power layer: raw ids, embedding info, provenance JSON.

**OKF interop:** Brain mode toolbar carries "Export brain as OKF bundle" and "Import bundle" (merge / replace / clone flows mirroring Clanker's semantics, with preview counts before any commit).

## 3. Review Mode (editorial desk)

Replaces `ReviewModal` entirely. Three columns:

- **Left — queue list:** proposal cards showing target entity, proposal type (new entity / update facts / new edges), source document names, and age. Oldest-first by default; filters by type and source. `j`/`k` navigate.
- **Center — proposal editor:** the proposal rendered as *what it will become*. A new entity shows a full entity-page preview, editable before approval. An update to an existing entity shows a **suggestion-style diff** — the current page with additions in green and removals in red, inline — with accept/reject per fact-level change, or direct text editing.

  Diffing must be **word-level (or semantic), not line-level**. LLMs rephrase rather than append: a librarian that subtly rewrites a paragraph to accommodate one new fact would render under a line-level differ as a wall of red followed by a wall of green — unreviewable. The diff library choice in the phase 2 plan must be evaluated against exactly this case (paragraph rewritten, one fact changed), and should fall back to a side-by-side old/new view when the computed diff exceeds a churn threshold (e.g. >70% of the paragraph changed) rather than showing noise.
- **Right — evidence panel:** the source chunks the librarian used, quoted verbatim, each with document name and line range; click opens Library at the exact spot. A "why this proposal" reasoning summary appears when the librarian recorded one.

**Actions (keyboard-first):** `a` approve, `r` reject, `e` focus editor, `space` next. Approve commits to Brain and advances. Multi-select enables batch approval of low-stakes proposals.

**Trust affordances:** every approval is logged as a Timeline event ("Approved: 3 facts added to *Project X* from *meeting-notes.pdf*"). Reject prompts for an optional reason, stored for future librarian tuning (not wired to the model yet).

**Empty state:** "Queue clear. Librarian watching 142 documents." plus last-synthesis time — never a blank void.

**Notifications:** live queue count badge on the rail; optional native OS notification when new proposals arrive (toggle in Settings → Librarian).

## 4. Library Mode (source documents)

**Sidebar:** folder tree of `documents/` (current FolderTree survives here). Each file row shows an ingestion-state icon: pending / chunked+embedded / synthesized / error, with folder-level rollup badges. Drag-drop anywhere in the app adds to Library — the current drop behavior promoted app-wide with a clear full-window overlay: "Drop to add to Library".

**Main — reader:** read-only document view (BlockNote read mode for markdown; PDF/DOCX render as extracted text with page/section markers). The protected badge is reframed: "Source document — read-only", with a tooltip explaining vault immutability. Deep-linkable to a chunk or line — Review evidence links and Brain fact chips land here with the target highlighted.

**Right panel — "What brain took from this":**

- Facts extracted from this document, grouped by entity (click → Brain).
- Pending proposals citing this document (click → Review).
- Chunk map: how the document was split, chunk count, embed status. Per-chunk detail lives in the power layer.

**Document actions:** re-ingest, exclude from synthesis (surfacing FolderRulesPanel logic contextually — right-click a folder → "Exclude from synthesis"), reveal in OS file manager.

**Ingestion transparency:** a newly dropped file animates through its states in the sidebar (pending → embedding → done). Errors show inline with a retry action, never buried.

## 5. Timeline & Tasks Modes

**Timeline mode:**

- Reverse-chronological event feed, grouped by day (matches OKF log granularity — date-stamped, not timestamped).
- Event types with distinct icons: ingested, synthesized, approved, rejected, healed, imported/exported, agent-access (MCP reads/writes).
- Filters sidebar: by type, entity, source, date range.
- Event rows are human sentences ("Learned 3 facts about *Project X* from *notes.pdf*"); click navigates to the target entity, document, or proposal.
- Layering: the default is a narrative feed; a power-layer toggle reveals raw event ids, MCP client names, and durations.
- Doubles as the audit log for MCP agent activity — "what did Cursor read/write yesterday" is answerable here.

**Tasks mode:**

- OKF tasks are actionable items extracted by the librarian or added by the user ("follow up with X", "verify claim Y").
- Task list with a status filter (open / done / archived), **grouped by parent entity by default** (with a group-by-source-document alternative). A week of meeting notes can produce 50+ extracted tasks; a purely flat list stops being scannable well before that. Grouping keeps it navigable without building kanban. Each task links to its entity and source.
- Manual creation supported; librarian-proposed tasks arrive through Review like facts do.
- Deliberately simple in v1: no due-date engine, no kanban, not project management.

**Activity feed (available from any mode):** a slide-over panel opened from the status bar or the rail pulse icon. The last ~50 events, live-updating, condensed, with an "Open full Timeline" link at the bottom. Same data as Timeline, ambient form.

## 6. Settings & Onboarding

**Settings** becomes a full-screen route (rail bottom icon) with left-nav tabs, replacing the stacked modal:

1. **Vault** — path, switcher, brain directory info (VaultPanel).
2. **Privacy** — see below.
3. **Models** — generation and embedding together: provider picker (Ollama sidecar / OpenAI-compatible URL), model dropdowns, health-check button, live status (GenerationPanel + EmbeddingPanel merged; ModelPanel/StepModel logic reused).
4. **Librarian** — synthesis cadence, folder rules (FolderRulesPanel), review-notification toggle. Auto-approve rules noted as future, not v1.
5. **Agents** — MCP config snippet and connected-client instructions (AgentIntegrationPanel).
6. **Maintenance** — health dashboard, re-embed, heal, prune (MaintenanceDashboard).
7. **Appearance** — theme light/dark/system. (BlockNote is currently hardcoded to `theme="light"` — fixed as part of this.)

### Privacy tab

A three-way privacy mode, presented as radio cards with plain-language consequences:

1. **Strict (default)** — fully local. Inference, embeddings, and storage all on-device. Cloud Bridge and external API fields disabled. "Nothing ever leaves this machine."
2. **Ephemeral cloud inference** — local storage and embeddings; generation may route to an external OpenAI-compatible API. Sent context is transient and never stored remotely. The UI shows exactly what gets sent (prompt + retrieved chunks) before first use.
3. **Connected agent (Cloud Bridge)** — the above plus the Clanker Cloud Bridge: your Clanker agent may **query** the vault on demand over a read-only channel (the five retrieval tools; see `2026-07-01-clanker-cloud-bridge-design.md`). Disclosure must describe what it actually is: individual query results leave the machine when the agent asks; **nothing syncs, nothing is stored remotely as a copy of the brain, and nothing can be written back over this channel**. *(Renamed from "Full cloud sync" — that label promised state synchronization the bridge does not do, and would have users consenting to the wrong mental model. If brain-state sync ever ships, it becomes a separate fourth mode or an explicit sub-toggle, not an expansion of this one.)*

**Enforcement, not just preference:** the mode gates the UI. Strict hides/disables cloud fields in the Models tab; the Cloud Bridge configuration (CloudBridgePanel) moves under the Privacy tab and is active only in mode 3. Downgrading to Strict prompts: "Disconnect cloud bridge and clear remote config?"

**Sequencing debt (flagged in the 2026-07-06 cross-repo architecture review):** the Cloud Bridge shipped (v1.8.0/v1.9.0) *before* this gating exists — today it is configurable regardless of privacy posture. The privacy-modes plan (split out of phase 6) must land before privacy modes are presented to users as enforced, and its migration step must handle the existing state: a user with a paired token but no chosen mode is placed in mode 3 (their current reality) and shown the disclosure, not silently downgraded to Strict with a live token in the keychain.

**Ambient indicator:** the status bar shows a privacy-mode shield glyph (filled / half / outline). Clicking it opens the Privacy tab. The user always knows the data posture at a glance.

### Onboarding (SetupWizard rework)

- Wizard steps reframed around outcomes: "Where's your stuff" (vault) → "Choose your privacy posture" (privacy mode — placed before model setup because it constrains provider choices) → "Pick your AI" (inference, with a "download Ollama for me" happy path) → "Watch it think".
- The final step ingests one sample document live, showing the chunk → embed → propose pipeline running, and lands the user in Review with their first real proposal. The last step teaches the core loop by doing it; no tour tooltips.
- Skippable for power users; re-runnable from Settings → Vault.

**First-run empty states:** every mode has a designed empty state pointing to the next action (Library: "Drop your first document"; Brain: "Approve your first proposal or create an entity"; and so on).

## 7. Cross-cutting Concerns

**Error handling:**

- All background failures surface in the Activity feed with a retry action — never a silent `console.error`. The current pattern of swallowed catches (EditorPane, ReviewModal) is eliminated.
- Blocking errors (vault unreadable, database corrupt) get full-screen recovery states with concrete actions, not a bare "Reload" button.
- Model or embedder down: the status-bar dot goes red and affected features degrade with an inline notice ("Search needs the embedder — check Models"); the app never hard-blocks reading.

**Visual direction** (implementation to follow the frontend-design skill at build time):

- Calm library aesthetic: quiet neutral surfaces, one accent color, generous text spacing — a reading app, not a dashboard.
- Dark and light themes both first-class.
- Comfortable density by default with a compact toggle (power layer).
- The current 116-line App.css is replaced by design tokens plus per-mode styles.

**Testing:** each phase keeps the existing test discipline — component tests per screen (following the existing `__tests__/` pattern), hooks tested standalone, and dedicated interaction tests for Review's keyboard flows.

## Phasing

Each phase gets its own implementation plan. Phases 1–2 are shippable immediately on the current data model; phases 4–5 were blocked on the backend spec (phase 3), now implemented in v1.10.0.

1. **Shell** — rail, modes, status bar, full-screen Settings. Current features rehomed; no data-model change. *(Implemented 2026-07-05.)*
2. **Review editorial desk** — three-column layout, keyboard flows, word-level diff component, per-item toggles, entity-aware diff. *(Structurally implemented 2026-07-05; V7 proposal API wiring Slice 1 merged PR #25; Slices 3–4 per-item toggles + entity diff implemented 2026-07-06.)*

### Phase 1 deferrals (shipped with documented gaps)

| Item | Deferred to |
|---|---|
| `⌘4` / `⌘5` mode shortcuts (Timeline, Tasks) | Phase 5 (modes not built yet) |
| Global `⌘K` command palette | Phase 1 follow-up or Phase 7 polish |
| Cross-mode links, back/forward history, peek panels | Cross-mode links + back/forward implemented Phase 4; peek panels → Phase 7 |
| Activity rail pulse icon | Phase 1 follow-up |
| Live Activity feed (beyond stub panel) | Phase 5 (Timeline data) |
| Per-mode empty states (Brain, Library) | Phase 4 / Phase 7 |
| Embedder/model down → inline feature notice | Phase 1 follow-up |
| Background errors → Activity feed + retry | Phase 5 |
| Review empty state richness (doc count, last-synthesis time) | Phase 2 |
| Library protected badge copy ("Source document — read-only") | Phase 4 |

### Phase 2 deferrals (V7 backend shipped; UI wiring in progress)

| Item | Status / target |
|---|---|
| Queue via `listProposals` (not legacy `get_review_queue` shim) | Implemented (Slice 1, `feat-phase-2-review-wiring`) |
| Approve/reject via `resolve_proposal` with per-item decisions | Implemented (Slice 3) |
| Evidence panel chunk quotes + line ranges | Implemented (Slice 1, `feat-phase-2-review-wiring`) |
| Entity-aware diff (`getEntity` + `ProposalDiff` for `update_entity`) | Implemented (Slice 4) |
| Per-fact accept/reject toggles in editor | Implemented (Slice 3) |
| Inline `edited_payload` editing | Follow-up after per-item toggles |
| Library deep-link from evidence | Implemented (Phase 4 navigation; opens document, chunk-level highlight → Phase 7) |

3. **Backend OKF-native migration** — schema, librarian synthesis output, event log. *(Implemented 2026-07-06, v1.10.0 — see `2026-07-05-okf-backend-migration-design.md`.)*
4. **Brain mode entity pages** — implemented 2026-07-06 (see `2026-07-06-phase-4-brain-entity-pages.md`). Deferred: peek panels, `[[` autocomplete, similarity in Connections, entity sort picker.
5. **Timeline + Tasks modes** — unblocked; not started.
6. **OKF import/export** — implemented 2026-07-06 (see `2026-07-06-phase-6-okf-bundle-io.md`). Privacy modes and Cloud Bridge gating split to a separate upcoming plan.
7. **Onboarding rework + visual polish pass.**

## Non-Goals (this vision, v1)

- Graph visualization view (deferred; backlinks + links cover navigation).
- Auto-approve rules for the librarian.
- Due dates, kanban, or any project-management depth in Tasks.
- Settings search.
- Wiring reject-reasons into librarian tuning (stored only).

## Open Questions Deferred to Follow-up Specs

- Exact OKF-native schema mapping in the Rust backend and migration path for existing vaults (phase 3 spec).
- Whether Curated Thoughts' OKF bundles need the same id-remap-on-clone semantics as Clanker's import (likely yes — decide in the phase 6 plan, referencing the Clanker import spec's collision-guard findings). *(Resolved: yes — OKF profile v1 §10 makes remap the application's job, and profile-1 bundles carry stable event ids that must be remapped too. See the backend spec's profile-v1 addendum, which also adds a phase 6 summary write-path decision.)*
- How librarian "reasoning summary" gets captured for the Review evidence panel (depends on librarian pipeline internals — phase 2/3 boundary).
