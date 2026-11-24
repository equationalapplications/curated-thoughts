import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { schemaOrgWarmAgentManifest } from '@equationalapplications/schema-org-llm-wiki';
import { schemaSoftwareOrgManifest } from '@equationalapplications/schema-software-org';
import { manifestFor, ontologyConfigFor } from '../lib/ontology';

type NodeType = { type: string; parent_type?: string };
type EdgeType = { type: string; source_type?: string; target_type?: string };

const nodes = schemaSoftwareOrgManifest.node_types as unknown as NodeType[];
const edges = schemaSoftwareOrgManifest.edge_types as unknown as EdgeType[];

const VENDORED_PATH = 'src-tauri/src/db/vendored/schema-software-org-9.18.0.manifest.json';

const childrenOf = (parent: string) =>
  nodes
    .filter((n) => n.parent_type === parent)
    .map((n) => n.type)
    .sort();

describe('schema packages', () => {
  it('schema-org ships 9 node types and 28 edge types', () => {
    expect(schemaOrgWarmAgentManifest.node_types).toHaveLength(9);
    expect(schemaOrgWarmAgentManifest.edge_types).toHaveLength(28);
  });

  it('schema-software-org ships 21 node types and 42 edge types', () => {
    expect(nodes).toHaveLength(21);
    expect(edges).toHaveLength(42);
  });

  it('creativework subtypes are the three documented children', () => {
    expect(childrenOf('creativework')).toEqual(['design_spec', 'procedure', 'reference_doc']);
  });

  it('statement is a root type whose children are decision, lesson, rule, trap', () => {
    const statement = nodes.find((n) => n.type === 'statement');
    expect(statement).toBeDefined();
    expect(statement?.parent_type).toBeUndefined();
    expect(childrenOf('statement')).toEqual(['decision', 'lesson', 'rule', 'trap']);
  });

  it('area exists as a node type', () => {
    expect(nodes.some((n) => n.type === 'area')).toBe(true);
  });

  it('statement about-edges target exactly area, organization, software_application', () => {
    const targets = edges
      .filter((e) => e.type === 'about' && e.source_type === 'statement')
      .map((e) => e.target_type)
      .sort();
    expect(targets).toEqual(['area', 'organization', 'software_application']);
  });

  it('handoff, session_recap and handoffFor are gone', () => {
    const slugs = nodes.map((n) => n.type);
    expect(slugs).not.toContain('handoff');
    expect(slugs).not.toContain('session_recap');
    expect(edges.some((e) => e.type === 'handoffFor')).toBe(false);
  });

  it('matches the vendored 9.18.0 snapshot the Rust migration embeds', () => {
    const vendored = JSON.parse(readFileSync(VENDORED_PATH, 'utf8'));
    expect(
      schemaSoftwareOrgManifest,
      `schema-software-org drifted from ${VENDORED_PATH}. Regenerate it: ` +
        `node --input-type=module -e "import {schemaSoftwareOrgManifest as m} from '@equationalapplications/schema-software-org'; ` +
        `import fs from 'fs'; fs.writeFileSync('${VENDORED_PATH}', JSON.stringify(m,null,2)+'\\n')"`,
    ).toEqual(vendored);
  });

  it('CT seed wrappers carry the real installed manifest', () => {
    expect(manifestFor('schema-software-org')).toBe(schemaSoftwareOrgManifest);
    const config = ontologyConfigFor('schema-software-org', ['e1', 'e2']);
    expect(config.mode).toBe('strict');
    expect(Object.keys(config.seedManifests ?? {}).sort()).toEqual(['e1', 'e2']);
    expect(config.seedManifests?.e1).toEqual({
      manifest: schemaSoftwareOrgManifest,
      mode: 'strict',
    });
  });
});
