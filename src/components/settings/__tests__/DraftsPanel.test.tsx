import "@testing-library/jest-dom";
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { WikiFact } from '@equationalapplications/react-llm-wiki';

type DraftsPage = { facts: WikiFact[]; nextCursor: string | null };

// `vi.fn()` with no type argument infers a callable signature that flows
// through `mockImplementationOnce` cleanly. We type the variables with
// `any`-casts on the assignment so strict-mode tsc doesn't reject the
// deferred Promise resolvers below.
const { listDrafts, enginePromoteDraft, promoteDraft } = vi.hoisted(() => ({
  listDrafts: vi.fn(),
  enginePromoteDraft: vi.fn(),
  promoteDraft: vi.fn(),
}));

vi.mock('../../../lib/wiki', () => ({
  wiki: { listDrafts, promoteDraft: enginePromoteDraft },
  seededOntologyEntityIds: () => ['tier_fact', 'tier_wisdom'],
}));
vi.mock('../../../lib/tauri', () => ({ promoteDraft }));

import { DraftsPanel } from '../DraftsPanel';

const fact = (id: string, title: string): WikiFact =>
  ({ id, title, body: '', entity_id: 'tier_fact' }) as WikiFact;

describe('DraftsPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listDrafts.mockImplementation(async (entityId: string) =>
      entityId === 'tier_fact'
        ? { facts: [fact('f1', 'Deploy runs on Fridays')], nextCursor: 'c1' }
        : { facts: [], nextCursor: null },
    );
    promoteDraft.mockResolvedValue(undefined);
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

  it('disables the per-fact Promote button while its promote is in flight', async () => {
    const deferred: { resolve?: () => void } = {};
    promoteDraft.mockImplementationOnce(
      () => new Promise<void>((resolve) => { deferred.resolve = resolve; }),
    );
    render(<DraftsPanel />);
    const promoteBtn = await screen.findByRole('button', { name: 'Promote Deploy runs on Fridays' });

    // First click starts the in-flight request and disables the button.
    await userEvent.click(promoteBtn);
    expect(promoteBtn).toBeDisabled();

    // A second click while still in flight must not issue a duplicate request.
    await userEvent.click(promoteBtn).catch(() => undefined);
    expect(promoteDraft).toHaveBeenCalledTimes(1);

    // Resolving the in-flight request re-enables the button (then removes the
    // row, so the assertion switches to the disappearing element).
    deferred.resolve?.();
    await waitFor(() => expect(screen.queryByText('Deploy runs on Fridays')).not.toBeInTheDocument());
  });

  it('disables the per-tier Load more button while its pagination is in flight', async () => {
    listDrafts.mockImplementationOnce(async () => ({ facts: [fact('f1', 'First')], nextCursor: 'c1' }))
      .mockImplementationOnce(async () => ({ facts: [], nextCursor: null }));
    const deferred: { resolve?: (page: DraftsPage) => void } = {};
    listDrafts.mockImplementationOnce(
      () => new Promise<DraftsPage>((resolve) => { deferred.resolve = resolve; }),
    );
    render(<DraftsPanel />);
    const moreBtn = await screen.findByRole('button', { name: 'Load more drafts for tier_fact' });

    await userEvent.click(moreBtn);
    expect(moreBtn).toBeDisabled();
    await userEvent.click(moreBtn).catch(() => undefined);
    // Only the initial load (×2 tiers) + the one loadMore must have happened.
    expect(listDrafts.mock.calls.filter((c) => 'cursor' in (c[1] ?? {}))).toHaveLength(1);

    deferred.resolve?.({ facts: [fact('f2', 'Second')], nextCursor: null });
    await waitFor(() => expect(screen.getByText('Second')).toBeInTheDocument());
  });
});
