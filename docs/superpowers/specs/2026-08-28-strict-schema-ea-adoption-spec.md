# Selectable Ontology Adoption — Spec

**Date:** 2026-08-28
**Status:** Draft — revised 2026-08-30 (PR #124 review, then reconciled against
the actual CT codebase; scope widened from "strict schema-software-org only" to
a selectable ontology with onboarding)
**Packages:** `curated-thoughts` (Tauri app + tools)
**Depends on:**
- `@equationalapplications/core-llm-wiki` >= 6.1.0 with parent field (2026-08-28-ontology-parent-field-spec)
- `@equationalapplications/schema-org-llm-wiki` (6.2.0)
- `@equationalapplications/schema-software-org` (6.2.0, 2026-08-28-schema-ea-executive-ontology-spec)
- Vault reorganization migration (vault-side doc)

---

## Executive Summary

CT gains a **selectable ontology**: one of two published schema packages,
emergent, or off. Desktop defaults to the general-purpose
`schema-org-llm-wiki`; the CLI defaults to `schema-software-org`. The chosen
manifest is seeded into the wiki engine in strict mode, so facts are
classified by a fixed type system. Separately, ingestion becomes
symlink-aware in a path-preserving way, so repo specs can live in their
source repos while remaining visible to Tessera's brain.

---

## Problem Statement

**CT has no ontology today.** `createWiki` is called in `src/lib/wiki.ts`
with only `llmProvider`, `config`, `onRetrievalFallback`, and `graphAdapter`
— there is no `ontology` option, no manifest seed, and no mode. Rust only
*reads* `llm_wiki_entity_manifests` (`src-tauri/src/wiki_graph.rs`, "no
mutation paths"), and the table's DDL default is empty:
`'{"node_types":[],"edge_types":[]}'` (`src-tauri/src/db/okf_ddl.rs:120`).
So facts are stored untyped, with no `okf_type` and no typed edges. This is
a greenfield seed, not a migration off a legacy manifest.

Two consequences shape this spec:

1. Because there is no incumbent manifest, there is also **no legacy manifest
   to fall back to** on failure — a bad manifest must fail loudly, not
   degrade.
2. Because the choice is greenfield, it can be made a **user choice** at
   onboarding rather than hardcoded. Different users want different type
   systems: a fiction author wants people/places/works; a software team wants
   specs/handoffs/services.

Additionally, the ingest walker's symlink handling recurses into the
**canonical target** (`tools/src/cmds.rs`, the `follow_symlinked_doc_dirs`
branch), so symlinked content is stored under target paths that leak outside
the vault and break vault-relative identity.

---

## Decisions

### D1: Strict mode for package ontologies, derived — not separately configured

Mode is **derived from the ontology selection**, never asked as its own
question:

| Selection | `OntologyConfig.mode` | Behavior |
|---|---|---|
| `schema-org-llm-wiki` | `strict` | Package manifest is authoritative |
| `schema-software-org` | `strict` | Package manifest is authoritative |
| `emergent` | `emergent` | Engine may propose new types |
| `off` | *(no manifest seeded)* | Facts stored untyped; no `okf_type`, no typed edges |

Under `strict`, the package schema is the authoritative type system — Tessera
never proposes ad-hoc types. Facts that don't fit any type get no `okf_type`
and no edges, rather than being forced into a wrong type.

`off` is distinct from "strict with no match": under `off` the extraction
pipeline runs normally and produces facts, embeddings, and search — only the
typed graph layer is inert. Retrieval and synthesis must not depend on
`okf_type` being present.

### D2: The manifest is seeded from TypeScript, not embedded in Rust

The extraction engine, `OntologyService`, and `validateManifest` all live in
`@equationalapplications/core-llm-wiki`, a TypeScript package. The schema
packages are likewise npm packages already importable from the frontend
workspace. Therefore the manifest is passed as an **`ontology` seed to
`createWiki` in `src/lib/wiki.ts`**, and persisted by the engine into
`llm_wiki_entity_manifests`.

There is **no `build.rs` step, no `include_str!`, and no Rust manifest
type.** An earlier draft of this spec proposed baking the manifest into the
Rust binary; that solved a language-boundary problem the architecture does
not have, and would have required inventing a Rust ontology service that
does not exist. Rust's role is unchanged: it reads
`llm_wiki_entity_manifests` for the MCP `wiki_get_ontology` tool.

Rust *does* own the **selection** (see D5) — a single config field read by
both the CLI and the Desktop app.

### D3: Path-preserving symlink resolution

Do **not** enable global `follow_links(true)` on the `WalkDir` instance: it
yields canonical target paths that break vault-relative identity, and exposes
the walker to symlink cycles.

`collect_files` keeps `follow_links(false)`. The existing selective-symlink
branch in `tools/src/cmds.rs` (symlinked directories that are direct children
of `<vault_root>/documents`) is **retained but corrected**: today it recurses
with `collect_files(&target, ...)` and therefore emits canonical target
paths. It must instead emit a **virtual path** — the original vault-relative
symlink prefix joined with the path relative to the resolved target — while
reading content from the real path.

This requires a **two-path contract** threaded through ingestion:

- `virtual_path` — vault-relative, symlink-prefix-preserving. Used for
  `documents.path`, chunk paths, and entity routing.
- `read_path` — the resolved real path. Used only to read bytes.

Guards on any selectively resolved symlink:

- **Containment:** the target must resolve inside the vault root *or* be a
  trusted link per D3a. (Plain vault containment cannot be the rule — repo
  specs live outside the vault by design.)
- **Depth:** the virtual path may not exceed a fixed depth budget
  (proposed: 16 segments) once the symlink prefix is applied.
- **Cycles:** nested symlinks inside a resolved target are never followed
  (the current code's approach — preserve it). This makes cycle detection
  unnecessary rather than merely unimplemented.
- **Broken links:** counted as ingest failures, not warned-and-skipped
  (see Risks).

### D3a: Containment is a trust-on-first-use ledger, not a user-authored list

**The stake is exfiltration, not tidiness.** CT sends ingested content to an
embedding provider and an LLM, which may be external
(`EmbedProfile::External`, `GenerationProviderKind::External`). A symlink at
`documents/notes -> ~/` would walk `~/.aws`, `~/.ssh`, and every `.env` in
every repo and ship them to a third party. The existing `is_excluded_file` /
`is_excluded_dir` rules are tuned for *noise* (lockfiles, generated dirs),
not for secrets, and must not be relied on as the boundary.

**No one types a path.** A user-authored roots list would re-introduce
exactly the kind of technical question D4 removes from onboarding, and it is
redundant in the happy case: creating the symlink is already a declaration of
intent. What that declaration does *not* establish is whether it is still
current, or whether the user made it at all (checkout and sync tools create
symlinks too). So the mechanism is a **freshness and authenticity check on
the symlink**, not an independent permission model.

**The ledger.** `BrainConfig` gains a `trusted_links` list, written by the
approval flows below and never hand-edited:

```jsonc
// config.json
{
  "trusted_links": [
    {
      // vault-relative path of the symlink itself
      "link": "documents/curated-thoughts-specs",
      // canonicalized target at the time of approval
      "target": "/Users/me/code/curated-thoughts/docs",
      "approved_at": 1756512000
    }
  ]
}
```

**Per-walk algorithm.** On every walk — not once at creation time — for each
direct-child symlink under `documents/`:

1. Canonicalize the target.
2. Look up the exact `(link, target)` pair in `trusted_links`.
3. **Exact pair match** → walk it, under the D3 guards.
4. **Unknown pair** — a new symlink, or an existing one whose target
   changed — → do not walk; record as pending and report it.

Matching the *pair* (not the target alone) is what makes repointing a
first-class event: a link silently repointed at a different directory
produces a fresh approval prompt rather than inheriting the old grant.

**Path comparison.** Both sides are canonicalized before comparison, and
containment tests compare **path components**, never string prefixes —
`~/code/proj` must not authorize `~/code/proj-secrets`.

**Approval by surface:**

- **Desktop:** pending links surface as a review prompt naming the resolved
  target — *"`documents/specs` now points to `~/code/foo/docs`. Include it in
  your brain?"* — one click to approve.
- **CLI / headless** (`ingest_vault_once`, MCP server): **deny by default**,
  with the exact remediation printed: `ct trust documents/specs`. A
  `--trust-new-links` flag exists for scripted setups that genuinely want
  blanket acceptance; it is never the default.

**Non-approvable denials.** These cannot be clicked or flagged through,
because approving them is always a mistake:

- the filesystem root, or the home directory itself
- any ancestor of the vault root, and any target containing the vault root
  (either direction makes the walker eat itself)
- any target that is an ancestor of an already-trusted target (prevents
  widening a grant by repointing one link upward)

A denied link is reported with the specific rule that rejected it.

**Deliberately excluded:** a "does this look like a repo?" heuristic (target
contains `.git`). It would reject legitimate non-repo targets such as a
research folder, and it does not reduce the exfiltration risk — a repo is
precisely where `.env` files live.

### D4: Minimal UI — a Settings panel, and no new required wizard step

Non-technical users must not be asked an ontology question to get started.
The Desktop setup wizard is already six steps; adding a seventh about type
systems is the wrong trade.

- **Desktop onboarding:** defaults to General
  (`schema-org-llm-wiki`) with **no blocking step**. The choice is surfaced
  as a preselected radio inside the existing `StepWelcome`, with the other
  three options behind a "Change" disclosure. Skipping is the happy path.
- **Settings:** a new `OntologyPanel.tsx` alongside the existing panels lets
  power users change the selection later, showing the npm package id as
  secondary text.
- **No schema-editing UI.** Users pick a schema; they never author or edit
  node/edge types in CT.

### D5: A single selection field shared by CLI and Desktop

The selection lives in `BrainConfig` (`src-tauri/src/config/mod.rs`) so both
surfaces read one source of truth:

```jsonc
// config.json
{
  "ontology": {
    // "schema-org" | "schema-software-org" | "emergent" | "off"
    "schema": "schema-org"
  }
}
```

Mode is derived (D1) and is not stored. The frontend reads the selection via
a Tauri command (`get_ontology_selection`) before calling `createWiki`, and
writes it via `set_ontology_selection`. `BrainConfig`'s existing
`preserved_keys` round-trip vehicle means older configs without the field
load cleanly and take the default.

**Defaults differ by surface, deliberately:**

- Desktop (`SetupWizard`): `schema-org` — general purpose, right for
  writers, researchers, and personal notes.
- CLI (`--onboard`): `schema-software-org` — the CLI is used from repos by
  engineers.

A config written by one surface is honored by the other; the differing
default applies only when the field is absent at onboarding time.

### D6: Switching ontology invalidates typed classifications

Changing the selection after ingestion leaves every existing `okf_type` and
typed edge derived from the *previous* manifest. Silently leaving a
half-classified graph is worse than not offering the switch.

On change, CT must:

1. Confirm explicitly, naming the consequence ("Existing type labels and
   connections will be rebuilt. Your notes, facts, and search are not
   affected.").
2. Clear `okf_type` and manifest-derived edges for affected entities.
3. Re-run the librarian to reclassify.

Facts, embeddings, chunks, and documents are never touched. Switching **to**
`off` clears typed data and does not reclassify; switching **from** `off`
is a plain first-time classification.

---

## Changes

### Dependency changes

- Bump `@equationalapplications/core-llm-wiki` **6.0.1 → >= 6.1.0**
  (`package.json`) for `parent_type` support.
- Bump `@equationalapplications/react-llm-wiki` in lockstep (currently
  6.0.1) — it must match the core version.
- Add `@equationalapplications/schema-org-llm-wiki@6.2.0`.
- Add `@equationalapplications/schema-software-org@6.2.0`.

Both schema packages ship the manifest as a JS export
(`schemaOrgLlmWikiManifest` / `schemaSoftwareOrgManifest`); no static
`manifest.json` is required, because the seed happens in TypeScript (D2).

### Ontology selection plumbing

**`src-tauri/src/config/mod.rs`** — add an `ontology` block to `BrainConfig`
with a `schema` field, plus `preserved_ontology` for unknown-key round-trip,
matching the existing `preserved_*` pattern.

**`src-tauri/src/onboard/mod.rs`** — add ontology selection to
`OnboardConfig` and to `collect_onboard_config`'s prompt flow, after the
generation-provider prompt:

```text
Knowledge schema (what kinds of things Tessera tracks):
  1) Software team  — specs, handoffs, services, procedures
  2) General        — people, places, events, works
  3) Let it invent its own
  4) None
Choice [1]:
```

**Tauri commands** — `get_ontology_selection` / `set_ontology_selection`.
The setter performs the D6 invalidation.

**`src/lib/wiki.ts`** — `makeWikiOptions` gains the resolved selection and
passes an `ontology` seed to `createWiki`. `setupWiki()` reads the selection
once, before the first `createWiki` call, and the two outbox listeners reuse
the already-resolved value rather than re-reading it (they must not race
against a Settings change).

**Which entities get seeded.** CT routes to at least three entity ids —
`tier_fact`, `tier_wisdom`, and `tier_working::<hash>` (`src/lib/wiki.ts`,
`src/lib/wikiTiers.ts`) — and `llm_wiki_entity_manifests` is keyed by
`entity_id`. The seed applies to **all three tiers**, and a newly minted
workspace entity is seeded at creation time from the current selection.

### UI

**`src/components/setup/StepWelcome.tsx`** — preselected "General" radio
with a "Change" disclosure exposing the other three. No new wizard step, no
change to `STEPS` length or `StepIndicator`.

**`src/components/settings/OntologyPanel.tsx`** (new) — the four options with
outcome-first labels, npm package id as secondary text, wired to
`set_ontology_selection` behind the D6 confirmation.

Labels used by both surfaces:

| Value | Label | Sub-label |
|---|---|---|
| `schema-org` | General | People, places, events, works |
| `schema-software-org` | Software team | Specs, handoffs, services |
| `emergent` | Let it invent its own | Types grow from your notes |
| `off` | None | Search and facts only, no typed graph |

### Symlink-aware ingestion

**File:** `tools/src/cmds.rs` (`collect_files`, the
`follow_symlinked_doc_dirs` branch)

The walker keeps `follow_links(false)`. The existing symlink branch is
changed to emit `(virtual_path, read_path)` pairs instead of pushing the
canonical target path, under the D3 guards.

**File:** `src-tauri/src/pipeline/mod.rs`

`entity_id_for_path` currently **canonicalizes its input** before stripping
the vault root, with a silent `unwrap_or_else` fallback. That resolves a
symlink path straight back to its target and loses the prefix — so fixing
`collect_files` alone is not sufficient. `entity_id_for_path`,
`ingest_file`, and `ingest_document_with_vault_root` must accept the
virtual path for identity and use the read path only for content.

The same applies to the TypeScript routing helper `entityIdForPath`
(`src/lib/wikiTiers.ts`), which must receive the virtual path.

### Trusted-link plumbing (D3a)

**`src-tauri/src/config/mod.rs`** — add `trusted_links: Vec<TrustedLink>`
to `BrainConfig` (`link`, `target`, `approved_at`), with a
`preserved_trusted_links` round-trip vehicle matching the existing
`preserved_*` pattern. Absent field → empty list → every symlink is pending.

**`tools/src/cmds.rs`** — `collect_files` takes the trusted-link ledger and
returns pending and denied links alongside files and errors, so callers can
report them. Denials name the rule that rejected them.

**`tools/src/bin/ct.rs`** — a `ct trust <vault-relative-link>` subcommand
that canonicalizes the current target, applies the non-approvable denial
rules, and appends the pair to the ledger. `ct trust --list` prints the
ledger; `ct trust --revoke <link>` removes an entry.

**Tauri commands** — `list_pending_links`, `approve_link`, `revoke_link`.
Approval is per-pair and re-required after a repoint.

**UI** — pending links appear in the existing review surface
(`src/components/review/`) rather than as a new screen; a vault with no
symlinks shows nothing and a non-technical user never encounters the
mechanism.

### Manifest validation and failure behavior

`validateManifest` (`core-llm-wiki`, `packages/core/src/utils/ontology.ts`)
runs on the seed path (`OntologyService` → `validateManifest(seed.manifest)`)
and on every manifest read (`MetadataRepository.getManifest`). Duplicate node
slugs, broken or over-deep `parent_type` chains, and bad edge keys throw at
startup, before any extraction runs.

Because CT has no incumbent manifest, there is nothing to fall back to:
**engine-init failure must surface as an app-boot failure**, with the
schema name in the message. A malformed manifest must never be swallowed
into an untyped-but-running state, because that is indistinguishable from a
deliberate `off` selection.

---

## What This Does NOT Include

- No changes to the watcher event types (still create/modify/delete)
- No changes to the embedding pipeline (still OpenRouter qwen3-embedding-4b)
- No changes to the folder_rules logic (existing `agents/` rule with
  synthesize + auto_approve still applies)
- No WikiLink parsing (WikiLinks are markdown — CT doesn't parse them)
- No changes to chunking, synthesis, or retrieval algorithms
- No schema-authoring UI — users select a published schema, never edit one
- No Rust-side ontology service or build-time manifest embedding
- No per-tier or per-folder ontology selection (one selection per vault)

---

## Risks

| Risk | Mitigation |
|------|-----------|
| Broken symlink is silently skipped | **Present-tense real:** `cmds.rs` prints a stderr warning and continues, and broken symlinks are not counted in `failed` (only `walk_errors` are). Fix: count broken tracked symlinks as ingest failures, plus a post-migration health check that resolves all tracked symlinks and fails loudly on any missing target. |
| Strict mode rejects valid facts | Types cover known vault content; the user can switch to `emergent` in Settings if gaps are found (D6 handles reclassification) |
| Switching ontology leaves a half-classified graph | D6: explicit confirmation, clear typed data, re-run librarian. Facts/embeddings untouched |
| Selection asked too early confuses non-technical users | D4: no blocking step; General is preselected and skipping is the happy path |
| CLI and Desktop defaults diverge into two behaviors | One shared `BrainConfig` field; the default differs only when the field is absent |
| Selectively resolved symlink ingests unexpected content | `follow_links(false)` retained; containment (vault or trusted link per D3a), depth budget, and no-nested-symlink rule per D3 |
| Symlinked target leaks secrets to an external embedding/LLM provider | D3a deny-by-default: an unapproved link is never walked. Noise exclusions are explicitly not treated as the security boundary |
| Trusted link is repointed at a different directory after approval | The ledger matches the `(link, target)` pair and is checked on every walk, so a repoint becomes a fresh pending approval, not an inherited grant |
| Headless runs silently skip content the user expected to be ingested | Pending and denied links are reported by `collect_files` and printed with the `ct trust` remediation; they are not warning-only |
| Manifest update requires a CT release | Acceptable — schema packages are versioned dependencies and change infrequently |

---

## Verification

1. Fresh Desktop onboarding completes **without** answering an ontology
   question, and the resulting `config.json` has `ontology.schema ==
   "schema-org"`.
2. Fresh CLI `--onboard` with all defaults accepted yields
   `ontology.schema == "schema-software-org"`.
3. After startup with a package schema selected, `wiki_get_ontology` returns
   a populated manifest for `tier_fact`, `tier_wisdom`, and the current
   workspace tier — not the empty DDL default.
4. Strict mode active — emergent type proposals rejected.
5. `off` selected → facts, chunks, embeddings, and search all work; no
   `okf_type` and no typed edges are written.
6. Ingest a design_spec file under `schema-software-org` → classified as
   `design_spec` with edges populated.
7. Query `creativework` parent type → returns the expected child types.
8. Ingest a test file via symlink → appears in DB. Assert `documents.path`
   stores the **vault-relative symlink path**, exactly preserving the
   prefix, and NOT the canonical target path. Assert the same for the
   derived `entity_id`.
9. Symlink whose target resolves outside the vault and is not in
   `trusted_links` → not walked; reported as pending with the `ct trust`
   remediation. `ct trust <link>` then makes the same file ingest on the
   next run.
10. Broken tracked symlink → run reports failure (not a warning-only skip).
11. A trusted link repointed at a different directory → not walked on the
    next run; reported as pending again, and the previously ingested content
    is not silently replaced.
12. Non-approvable targets (`/`, the home directory, an ancestor of the
    vault root, a directory containing the vault root, an ancestor of an
    already-trusted target) → refused by both `ct trust` and the Desktop
    approval flow, naming the rule.
13. Component-wise containment: with `~/code/proj` trusted, a link targeting
    `~/code/proj-secrets` is still pending, not auto-approved by string
    prefix.
14. `--trust-new-links` approves pending links in a scripted run; without
    it, a headless run denies and exits non-zero on pending links.
15. Switch ontology in Settings → confirmation shown; after confirming,
    typed classifications are cleared and rebuilt; fact count and embedding
    count are unchanged.
16. Semantically corrupt manifest (duplicate node type, broken
    `parent_type`) → `validateManifest` throws at engine startup and CT
    surfaces an app-boot failure naming the schema; it does not start
    untyped.
