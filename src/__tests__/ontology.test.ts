import { describe, expect, it } from 'vitest';
import { schemaOrgWarmAgentManifest } from '@equationalapplications/schema-org-llm-wiki';
import { schemaSoftwareOrgManifest } from '@equationalapplications/schema-software-org';
import { ONTOLOGY_OPTIONS, manifestFor, ontologyConfigFor } from '../lib/ontology';

const ENTITIES = ['tier_fact', 'tier_wisdom', 'tier_working::abc123'];

describe('ontologyConfigFor', () => {
  it('seeds every entity with the software-org manifest in strict mode', () => {
    const cfg = ontologyConfigFor('schema-software-org', ENTITIES);
    expect(cfg.mode).toBe('strict');
    expect(Object.keys(cfg.seedManifests ?? {})).toEqual(ENTITIES);
    for (const id of ENTITIES) {
      expect(cfg.seedManifests?.[id].manifest).toBe(schemaSoftwareOrgManifest);
      expect(cfg.seedManifests?.[id].mode).toBe('strict');
    }
  });

  it('seeds the general manifest for schema-org', () => {
    const cfg = ontologyConfigFor('schema-org', ENTITIES);
    expect(cfg.mode).toBe('strict');
    expect(cfg.seedManifests?.tier_fact.manifest).toBe(schemaOrgWarmAgentManifest);
  });

  it('emergent seeds no manifest and runs in emergent mode', () => {
    const cfg = ontologyConfigFor('emergent', ENTITIES);
    expect(cfg.mode).toBe('emergent');
    expect(cfg.seedManifests).toBeUndefined();
  });

  it('off is an explicit engine mode, not an omission', () => {
    const cfg = ontologyConfigFor('off', ENTITIES);
    expect(cfg.mode).toBe('off');
    expect(cfg.seedManifests).toBeUndefined();
  });

  it('manifestFor returns null for the manifest-less selections', () => {
    expect(manifestFor('emergent')).toBeNull();
    expect(manifestFor('off')).toBeNull();
    expect(manifestFor('schema-org')).toBe(schemaOrgWarmAgentManifest);
  });

  it('exposes the four options with the agreed copy', () => {
    expect(ONTOLOGY_OPTIONS.map((o) => o.value)).toEqual([
      'schema-org',
      'schema-software-org',
      'emergent',
      'off',
    ]);
    expect(ONTOLOGY_OPTIONS[0].label).toBe('General');
    expect(ONTOLOGY_OPTIONS[1].label).toBe('Software team');
  });
});