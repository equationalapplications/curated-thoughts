import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { OntologyPanel } from '../components/settings/OntologyPanel';
import { setOntologySelection } from '../lib/tauri';
import { wiki } from '../lib/wiki';

vi.mock('../lib/tauri', () => ({
  getOntologySelection: vi.fn().mockResolvedValue('schema-org'),
  setOntologySelection: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('../lib/wiki', () => ({
  wiki: {
    setOntologyManifest: vi.fn().mockResolvedValue(undefined),
    runOntologyBackfill: vi.fn().mockResolvedValue({ remaining: 0, typed: 3, scanned: 3 }),
  },
  getWorkspaceId: () => 'tier_working::abc123',
}));

describe('OntologyPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.spyOn(window, 'confirm').mockReturnValue(true);
  });

  it('renders all four options with the package id as secondary text', async () => {
    render(<OntologyPanel />);
    expect(await screen.findByRole('radio', { name: /General/ })).toBeChecked();
    expect(screen.getByText('@equationalapplications/schema-software-org')).toBeInTheDocument();
  });

  it('confirms before switching, then reseeds every tier and backfills', async () => {
    render(<OntologyPanel />);
    fireEvent.click(await screen.findByRole('radio', { name: /Software team/ }));

    await waitFor(() => expect(window.confirm).toHaveBeenCalled());
    await waitFor(() =>
      expect(setOntologySelection).toHaveBeenCalledWith('schema-software-org'),
    );

    const seeded = vi.mocked(wiki.setOntologyManifest).mock.calls.map((c) => c[0]);
    expect(seeded).toEqual(['tier_fact', 'tier_wisdom', 'tier_working::abc123']);
    expect(wiki.runOntologyBackfill).toHaveBeenCalledTimes(3);
  });

  it('makes no changes when the confirmation is declined', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    render(<OntologyPanel />);
    fireEvent.click(await screen.findByRole('radio', { name: /Software team/ }));

    await waitFor(() => expect(window.confirm).toHaveBeenCalled());
    expect(setOntologySelection).not.toHaveBeenCalled();
    expect(wiki.setOntologyManifest).not.toHaveBeenCalled();
  });

  it('loops backfill until nothing remains', async () => {
    vi.mocked(wiki.runOntologyBackfill)
      .mockResolvedValueOnce({ remaining: 2, typed: 1, scanned: 3 } as never)
      .mockResolvedValue({ remaining: 0, typed: 2, scanned: 2 } as never);

    render(<OntologyPanel />);
    fireEvent.click(await screen.findByRole('radio', { name: /None/ }));

    // 3 tiers, first of which needs a second pass.
    await waitFor(() => expect(wiki.runOntologyBackfill).toHaveBeenCalledTimes(4));
  });
});
