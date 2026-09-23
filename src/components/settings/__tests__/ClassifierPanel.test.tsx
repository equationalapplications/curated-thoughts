import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const { getClassifierConfig, setClassifierConfig, usePrivacyMode } = vi.hoisted(() => ({
  getClassifierConfig: vi.fn(),
  setClassifierConfig: vi.fn().mockResolvedValue(undefined),
  usePrivacyMode: vi.fn(() => ({ mode: 'ephemeral' })),
}));
vi.mock('../../../lib/tauri', () => ({ getClassifierConfig, setClassifierConfig }));
vi.mock('../../../hooks/usePrivacyMode', () => ({ usePrivacyMode }));

import { ClassifierPanel } from '../ClassifierPanel';

describe('ClassifierPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    usePrivacyMode.mockReturnValue({ mode: 'ephemeral' });
    getClassifierConfig.mockResolvedValue({ provider: 'unconfigured' });
  });

  it('discloses that fact text leaves the device', async () => {
    render(<ClassifierPanel />);
    expect(await screen.findByText(/fact titles and bodies are sent/i)).toBeInTheDocument();
  });

  it('saves a Cloudflare config with snake_case keys', async () => {
    render(<ClassifierPanel />);
    await userEvent.selectOptions(await screen.findByLabelText('Classifier provider'), 'cloudflare_jev');
    await userEvent.type(screen.getByLabelText('Cloudflare account ID'), 'abc123');
    await userEvent.type(screen.getByLabelText('API token'), 'tok');
    await userEvent.click(screen.getByRole('button', { name: 'Save classifier' }));
    expect(setClassifierConfig).toHaveBeenCalledWith({
      provider: 'cloudflare_jev',
      url: null,
      account_id: 'abc123',
      api_key: 'tok',
      min_confidence: 0.5,
      timeout_secs: null,
    });
  });

  it('preserves the loaded timeout_secs across save', async () => {
    getClassifierConfig.mockResolvedValue({ provider: 'cloudflare_jev', account_id: 'abc123', timeout_secs: 45, has_api_key: false });
    render(<ClassifierPanel />);
    // The timeout input should reflect the loaded value (45) once the
    // provider is non-unconfigured (the input is gated on that to avoid
    // showing classifier-only fields when no provider is chosen).
    expect(await screen.findByLabelText('Request timeout (seconds)')).toHaveValue(45);
    await userEvent.click(screen.getByRole('button', { name: 'Save classifier' }));
    expect(setClassifierConfig).toHaveBeenCalledWith(
      expect.objectContaining({ timeout_secs: 45 }),
    );
  });

  it('renders an explicit Clear stored token button only when a key is already stored', async () => {
    getClassifierConfig.mockResolvedValue({
      provider: 'cloudflare_jev',
      account_id: 'abc123',
      has_api_key: true,
    });
    render(<ClassifierPanel />);
    await screen.findByText(/a token is currently stored/i);
    const clearBtn = await screen.findByRole('button', { name: 'Clear stored token' });
    await userEvent.click(clearBtn);
    await waitFor(() =>
      expect(setClassifierConfig).toHaveBeenCalledWith(
        expect.objectContaining({ api_key: '' }),
      ),
    );
  });

  it('disables controls until the initial getClassifierConfig resolves', async () => {
    let resolveConfig: ((v: unknown) => void) | null = null;
    getClassifierConfig.mockReturnValue(
      new Promise((resolve) => {
        resolveConfig = resolve;
      }),
    );
    render(<ClassifierPanel />);
    const providerSelect = screen.getByLabelText('Classifier provider');
    expect(providerSelect).toBeDisabled();
    resolveConfig?.({ provider: 'unconfigured' });
    await waitFor(() => expect(providerSelect).not.toBeDisabled());
  });

  it('is disabled with an explanation in strict mode', async () => {
    usePrivacyMode.mockReturnValue({ mode: 'strict' });
    render(<ClassifierPanel />);
    expect(await screen.findByLabelText('Classifier provider')).toBeDisabled();
    expect(screen.getByText(/Strict privacy blocks the classifier/)).toBeInTheDocument();
  });

  it('shows a save error', async () => {
    setClassifierConfig.mockRejectedValueOnce('jev_http requires an http(s) url');
    render(<ClassifierPanel />);
    await userEvent.selectOptions(await screen.findByLabelText('Classifier provider'), 'jev_http');
    await userEvent.click(screen.getByRole('button', { name: 'Save classifier' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('http(s) url');
  });
});
