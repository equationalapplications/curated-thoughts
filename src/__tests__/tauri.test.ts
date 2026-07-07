import { vi, describe, it, expect, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  runWikiHeal,
  runWikiPrune,
  runWikiReembed,
  forgetWikiSource,
  subscribeEntityStatus,
  getEntityConnections,
  addEntityFact,
  updateEntityFact,
  archiveEntityFact,
} from '../lib/tauri';

describe('tauri API helpers', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('subscribes to wiki status events', async () => {
    const callback = vi.fn();
    await subscribeEntityStatus(callback);
    expect(listen).toHaveBeenCalledWith('wiki-status-change', callback);
  });

  it('runs heal, prune, and reembed commands', async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await runWikiHeal();
    expect(invoke).toHaveBeenCalledWith('run_wiki_heal');

    await runWikiPrune();
    expect(invoke).toHaveBeenCalledWith('run_wiki_prune');

    await runWikiReembed();
    expect(invoke).toHaveBeenCalledWith('run_wiki_reembed');
  });

  it('forgets a wiki source path through the proper command', async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await forgetWikiSource('/Users/test/Vault/documents/notes.md');
    expect(invoke).toHaveBeenCalledWith('run_wiki_forget', {
      sourcePath: '/Users/test/Vault/documents/notes.md',
    });
  });

  it('entity connections and fact CRUD bindings call the right commands', async () => {
    vi.mocked(invoke).mockResolvedValue(null);

    await getEntityConnections('ent_1');
    expect(invoke).toHaveBeenCalledWith('get_entity_connections_cmd', { entityId: 'ent_1' });

    await addEntityFact('ent_1', 'A body.');
    expect(invoke).toHaveBeenCalledWith('add_entity_fact_cmd', { entityId: 'ent_1', body: 'A body.' });

    await updateEntityFact('ent_1', 'fact_1', 'New body.');
    expect(invoke).toHaveBeenCalledWith('update_entity_fact_cmd', {
      entityId: 'ent_1',
      factId: 'fact_1',
      body: 'New body.',
    });

    await archiveEntityFact('ent_1', 'fact_1');
    expect(invoke).toHaveBeenCalledWith('archive_entity_fact_cmd', { entityId: 'ent_1', factId: 'fact_1' });
  });
});
