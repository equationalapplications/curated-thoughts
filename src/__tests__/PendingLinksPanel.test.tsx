import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { PendingLinksPanel } from '../components/review/PendingLinksPanel';
import { approveLink, listPendingLinks } from '../lib/tauri';

vi.mock('../lib/tauri', () => ({
  listPendingLinks: vi.fn(),
  approveLink: vi.fn().mockResolvedValue(undefined),
}));

describe('PendingLinksPanel', () => {
  beforeEach(() => vi.clearAllMocks());

  it('renders nothing when there are no pending links', async () => {
    vi.mocked(listPendingLinks).mockResolvedValue([]);
    const { container } = render(<PendingLinksPanel />);
    await waitFor(() => expect(listPendingLinks).toHaveBeenCalled());
    expect(container.textContent).toBe('');
  });

  it('names the link and its resolved target', async () => {
    vi.mocked(listPendingLinks).mockResolvedValue([
      { link: 'documents/specs', target: '/Users/me/code/foo/docs' },
    ]);
    render(<PendingLinksPanel />);
    expect(await screen.findByText(/documents\/specs/)).toBeInTheDocument();
    expect(screen.getByText('/Users/me/code/foo/docs')).toBeInTheDocument();
  });

  it('approves a link and drops it from the list', async () => {
    vi.mocked(listPendingLinks)
      .mockResolvedValueOnce([{ link: 'documents/specs', target: '/x/docs' }])
      .mockResolvedValueOnce([]);
    render(<PendingLinksPanel />);
    fireEvent.click(await screen.findByRole('button', { name: /include/i }));
    await waitFor(() => expect(approveLink).toHaveBeenCalledWith('documents/specs'));
    await waitFor(() => expect(screen.queryByText(/documents\/specs/)).not.toBeInTheDocument());
  });
});
