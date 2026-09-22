import "@testing-library/jest-dom";
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const { listDrafts, enginePromoteDraft, promoteDraft } = vi.hoisted(() => ({
  listDrafts: vi.fn(),
  enginePromoteDraft: vi.fn(),
  promoteDraft: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('../../../lib/wiki', () => ({
  wiki: { listDrafts, promoteDraft: enginePromoteDraft },
  seededOntologyEntityIds: () => ['tier_fact', 'tier_wisdom'],
}));
vi.mock('../../../lib/tauri', () => ({ promoteDraft }));

import { DraftsPanel } from '../DraftsPanel';

const fact = (id: string, title: string) => ({ id, title, body: '', entity_id: 'tier_fact' });

describe('DraftsPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listDrafts.mockImplementation(async (entityId: string) =>
      entityId === 'tier_fact'
        ? { facts: [fact('f1', 'Deploy runs on Fridays')], nextCursor: 'c1' }
        : { facts: [], nextCursor: null },
    );
  });

  it('lists drafts per seeded tier', async () => {
    render(<DraftsPanel />);
    expect(await screen.findByText('Deploy runs on Fridays')).toBeInTheDocument();
    expect(listDrafts).toHaveBeenCalledWith('tier_fact', { limit: 20 });
    expect(listDrafts).toHaveBeenCalledWith('tier_wisdom', { limit: 20 });
  });

  it('promotes through the Rust command, never the engine', async () => {
    render(<DraftsPanel />);
    await userEvent.click(await screen.findByRole('button', { name: 'Promote Deploy runs on Fridays' }));
    expect(promoteDraft).toHaveBeenCalledWith('f1', 'tier_fact');
    expect(enginePromoteDraft).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByText('Deploy runs on Fridays')).not.toBeInTheDocument());
  });

  it('loads the next page with the cursor', async () => {
    listDrafts.mockImplementationOnce(async () => ({ facts: [fact('f1', 'First')], nextCursor: 'c1' }))
      .mockImplementationOnce(async () => ({ facts: [], nextCursor: null }))
      .mockImplementationOnce(async () => ({ facts: [fact('f2', 'Second')], nextCursor: null }));
    render(<DraftsPanel />);
    await userEvent.click(await screen.findByRole('button', { name: 'Load more drafts for tier_fact' }));
    expect(listDrafts).toHaveBeenLastCalledWith('tier_fact', { limit: 20, cursor: 'c1' });
    expect(await screen.findByText('Second')).toBeInTheDocument();
  });

  it('shows a promote failure', async () => {
    promoteDraft.mockRejectedValueOnce('not_draft');
    render(<DraftsPanel />);
    await userEvent.click(await screen.findByRole('button', { name: 'Promote Deploy runs on Fridays' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('not_draft');
  });
});
