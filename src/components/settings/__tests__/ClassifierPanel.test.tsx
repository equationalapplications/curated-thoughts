import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

// `vi.fn()` with no explicit type argument lets `mockImplementationOnce`
// accept a `(...) => Promise<T>` without strict-mode tsc collapsing the
// callback to `never`. We assert on `mock.calls` directly when needed.
const { getClassifierConfig, setClassifierConfig, usePrivacyMode } = vi.hoisted(() => ({
  getClassifierConfig: vi.fn(),
  setClassifierConfig: vi.fn(),
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
    setClassifierConfig.mockResolvedValue(undefined);
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
      model: null,
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
    const deferred: { resolve?: (cfg: { provider: 'unconfigured' }) => void } = {};
    getClassifierConfig.mockImplementationOnce(
      () => new Promise<{ provider: 'unconfigured' }>((resolve) => { deferred.resolve = resolve; }),
    );
    render(<ClassifierPanel />);
    const providerSelect = screen.getByLabelText('Classifier provider');
    expect(providerSelect).toBeDisabled();
    deferred.resolve?.({ provider: 'unconfigured' });
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

  it('saves the typed jev_http model field exactly as typed (blank = unpin)', async () => {
    render(<ClassifierPanel />);
    await userEvent.selectOptions(await screen.findByLabelText('Classifier provider'), 'jev_http');
    await userEvent.type(screen.getByLabelText('Endpoint URL'), 'https://jev.example.com');
    await userEvent.type(screen.getByLabelText('Model'), 'jev-small');
    await userEvent.click(screen.getByRole('button', { name: 'Save classifier' }));
    expect(setClassifierConfig).toHaveBeenCalledWith({
      provider: 'jev_http',
      url: 'https://jev.example.com',
      account_id: null,
      api_key: null,
      min_confidence: 0.5,
      timeout_secs: null,
      model: 'jev-small',
    });
    // Blank model field is sent as "" (explicit unpin signal), not
    // normalized to null by the panel.
    await userEvent.clear(screen.getByLabelText('Model'));
    await userEvent.click(screen.getByRole('button', { name: 'Save classifier' }));
    expect(setClassifierConfig).toHaveBeenLastCalledWith(
      expect.objectContaining({ model: '' }),
    );
  });

  it('does not send the model field for cloudflare_jev saves', async () => {
    render(<ClassifierPanel />);
    await userEvent.selectOptions(await screen.findByLabelText('Classifier provider'), 'cloudflare_jev');
    await userEvent.type(screen.getByLabelText('Cloudflare account ID'), 'abc123');
    await userEvent.click(screen.getByRole('button', { name: 'Save classifier' }));
    expect(setClassifierConfig).toHaveBeenCalledWith(
      expect.objectContaining({ model: null }),
    );
  });

  it('sends model: null when switching to cloudflare with a loaded pin (merge_stored treats null as untouched, so the stored pin survives server-side)', async () => {
    // Seed the panel WITH a saved jev_http pin so the load effect hydrates
    // model state, THEN switch provider to cloudflare_jev and save. The UI
    // contract here is the null payload only — null means "untouched" to
    // merge_stored (tested in Rust), so the stored pin is preserved on the
    // backend rather than cleared by the panel.
    getClassifierConfig.mockResolvedValue({
      provider: 'jev_http',
      url: 'https://jev.example.com',
      model: 'jev-small',
      has_api_key: false,
    });
    render(<ClassifierPanel />);
    const modelInput = await screen.findByLabelText('Model');
    expect(modelInput).toHaveValue('jev-small');
    await userEvent.selectOptions(screen.getByLabelText('Classifier provider'), 'cloudflare_jev');
    await userEvent.click(screen.getByRole('button', { name: 'Save classifier' }));
    expect(setClassifierConfig).toHaveBeenCalledWith(
      expect.objectContaining({ model: null }),
    );
  });
});
