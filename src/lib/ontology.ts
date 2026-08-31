import type {
  OntologyConfig,
  OntologyManifest,
  OntologyMode,
} from '@equationalapplications/core-llm-wiki';
import { schemaOrgWarmAgentManifest } from '@equationalapplications/schema-org-llm-wiki';
import { schemaSoftwareOrgManifest } from '@equationalapplications/schema-software-org';
import type { OntologySelection } from './tauri';

export interface OntologyOption {
  value: OntologySelection;
  label: string;
  subLabel: string;
  /** npm package id, shown as secondary text in Settings. Empty when N/A. */
  packageId: string;
}

/** Outcome-first copy. Shared verbatim by the wizard and the Settings panel. */
export const ONTOLOGY_OPTIONS: OntologyOption[] = [
  {
    value: 'schema-org',
    label: 'General',
    subLabel: 'People, places, events, works',
    packageId: '@equationalapplications/schema-org-llm-wiki',
  },
  {
    value: 'schema-software-org',
    label: 'Software team',
    subLabel: 'Specs, handoffs, services',
    packageId: '@equationalapplications/schema-software-org',
  },
  {
    value: 'emergent',
    label: 'Let it invent its own',
    subLabel: 'Types grow from your notes',
    packageId: '',
  },
  {
    value: 'off',
    label: 'None',
    subLabel: 'Search and facts only, no typed graph',
    packageId: '',
  },
];

/** The manifest a selection seeds, or null when it seeds none. */
export function manifestFor(selection: OntologySelection): OntologyManifest | null {
  switch (selection) {
    case 'schema-org':
      return schemaOrgWarmAgentManifest;
    case 'schema-software-org':
      return schemaSoftwareOrgManifest;
    default:
      return null;
  }
}

/**
 * Mode is derived from the selection, never configured separately (spec D1).
 * A package selection implies strict; the other two name their own mode.
 */
export function modeFor(selection: OntologySelection): OntologyMode {
  switch (selection) {
    case 'schema-org':
    case 'schema-software-org':
      return 'strict';
    case 'emergent':
      return 'emergent';
    case 'off':
      return 'off';
  }
}

/**
 * Build the engine's ontology config. `seedManifests` is written to the DB only
 * when an entity has no row yet, so this bootstraps fresh vaults; switching an
 * existing vault goes through `setOntologyManifest` (see OntologyPanel).
 */
export function ontologyConfigFor(
  selection: OntologySelection,
  entityIds: string[],
): OntologyConfig {
  const mode = modeFor(selection);
  const manifest = manifestFor(selection);
  if (!manifest) return { mode };

  const seedManifests: NonNullable<OntologyConfig['seedManifests']> = {};
  for (const id of entityIds) {
    seedManifests[id] = { manifest, mode };
  }
  return { mode, seedManifests };
}