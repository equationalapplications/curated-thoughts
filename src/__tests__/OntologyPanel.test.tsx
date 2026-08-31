import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { OntologyPanel } from '../components/settings/OntologyPanel';
import { setOntologySelection } from '../lib/tauri';
import { applyOntologyChange } from '../lib/wiki';

vi.mock('../lib/tauri', () => ({
  getOntologySelection: vi.fn().mockResolvedValue('schema-org'),
  setOntologySelection: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('../lib/wiki', () => ({
  applyOntologyChange: vi.fn().mockResolvedValue(undefined),
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

  it('confirms before switching, then persists and applies the D6 sequence', async () => {
    render(<OntologyPanel />);
    fireEvent.click(await screen.findByRole('radio', { name: /Software team/ }));

    await waitFor(() => expect(window.confirm).toHaveBeenCalled());
    await waitFor(() =>
      expect(setOntologySelection).toHaveBeenCalledWith('schema-software-org'),
    );
    await waitFor(() =>
      expect(applyOntologyChange).toHaveBeenCalledWith('schema-software-org'),
    );
  });

  it('makes no changes when the confirmation is declined', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    render(<OntologyPanel />);
    fireEvent.click(await screen.findByRole('radio', { name: /Software team/ }));

    await waitFor(() => expect(window.confirm).toHaveBeenCalled());
    expect(setOntologySelection).not.toHaveBeenCalled();
    expect(applyOntologyChange).not.toHaveBeenCalled();
  });

  it('restores the prior selection when the D6 sequence fails', async () => {
    vi.mocked(applyOntologyChange).mockRejectedValueOnce(
      new Error('manifest reseed failed') as never,
    );

    render(<OntologyPanel />);
    fireEvent.click(await screen.findByRole('radio', { name: /Software team/ }));

    await waitFor(() =>
      expect(setOntologySelection).toHaveBeenCalledWith('schema-software-org'),
    );
    await waitFor(() =>
      expect(setOntologySelection).toHaveBeenLastCalledWith('schema-org'),
    );
    expect(await screen.findByText(/manifest reseed failed/)).toBeInTheDocument();
  });
});
