import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { OntologyChoice } from '../components/setup/OntologyChoice';
import { setOntologySelection } from '../lib/tauri';
import { wiki } from '../lib/wiki';

vi.mock('../lib/tauri', () => ({
  getOntologySelection: vi.fn().mockResolvedValue('schema-org'),
  setOntologySelection: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('../lib/wiki', () => ({
  wiki: {
    setOntologyManifest: vi.fn().mockResolvedValue(undefined),
    runOntologyBackfill: vi.fn().mockResolvedValue({ remaining: 0, typed: 0, scanned: 0 }),
  },
  applyOntologyChange: vi.fn(async () => {
    await wiki.setOntologyManifest('tier_fact', { node_types: [], edge_types: [] }, { mode: 'off' });
  }),
}));

describe('OntologyChoice', () => {
  beforeEach(() => vi.clearAllMocks());

  it('shows General preselected and hides the other options', async () => {
    render(<OntologyChoice />);
    const general = await screen.findByRole('radio', { name: /General/ });
    expect(general).toBeChecked();
    expect(screen.queryByRole('radio', { name: /Software team/ })).not.toBeInTheDocument();
  });

  it('reveals all four options behind the Change disclosure', async () => {
    render(<OntologyChoice />);
    fireEvent.click(await screen.findByRole('button', { name: /change/i }));
    expect(screen.getByRole('radio', { name: /Software team/ })).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: /Let it invent its own/ })).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: /None/ })).toBeInTheDocument();
  });

  it('persists a changed selection', async () => {
    render(<OntologyChoice />);
    fireEvent.click(await screen.findByRole('button', { name: /change/i }));
    fireEvent.click(screen.getByRole('radio', { name: /Software team/ }));
    await waitFor(() =>
      expect(setOntologySelection).toHaveBeenCalledWith('schema-software-org'),
    );
  });

  it('does not persist anything when the user just moves on', async () => {
    render(<OntologyChoice />);
    await screen.findByRole('radio', { name: /General/ });
    expect(setOntologySelection).not.toHaveBeenCalled();
  });

  it('keeps the prior radio checked when persistence fails', async () => {
    vi.mocked(setOntologySelection).mockRejectedValueOnce(new Error('save failed'));
    render(<OntologyChoice />);
    fireEvent.click(await screen.findByRole('button', { name: /change/i }));
    const software = screen.getByRole('radio', { name: /Software team/ });
    fireEvent.click(software);

    await waitFor(() => expect(setOntologySelection).toHaveBeenCalled());
    expect(software).not.toBeChecked();
    expect(screen.getByRole('radio', { name: /General/ })).toBeChecked();
    expect(screen.getByText(/save failed/i)).toBeInTheDocument();
  });
});
