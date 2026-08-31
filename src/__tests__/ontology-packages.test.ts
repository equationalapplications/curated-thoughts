import { describe, expect, it } from 'vitest';
import { schemaOrgWarmAgentManifest } from '@equationalapplications/schema-org-llm-wiki';
import { schemaSoftwareOrgManifest } from '@equationalapplications/schema-software-org';

describe('schema packages', () => {
  it('schema-org ships 9 node types and 28 edge types', () => {
    expect(schemaOrgWarmAgentManifest.node_types).toHaveLength(9);
    expect(schemaOrgWarmAgentManifest.edge_types).toHaveLength(28);
  });

  it('schema-software-org ships 17 node types and 40 edge types', () => {
    expect(schemaSoftwareOrgManifest.node_types).toHaveLength(17);
    expect(schemaSoftwareOrgManifest.edge_types).toHaveLength(40);
  });

  it('creativework subtypes are the five documented children', () => {
    const children = schemaSoftwareOrgManifest.node_types
      .filter((n) => (n as { parent_type?: string }).parent_type === 'creativework')
      .map((n) => n.type)
      .sort();
    expect(children).toEqual([
      'design_spec',
      'handoff',
      'procedure',
      'reference_doc',
      'session_recap',
    ]);
  });
});