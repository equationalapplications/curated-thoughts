import { render, screen, waitFor, fireEvent, act } from "@testing-library/react";
import { vi, describe, it, expect, beforeEach } from "vitest";

const privacyMock = vi.hoisted(() => ({
  mode: "ephemeral" as "strict" | "ephemeral" | "connected",
  ephemeral_disclosure_acknowledged: true,
}));

import { GenerationPanel } from "../components/settings/GenerationPanel";

// Mock the tauri module
vi.mock("../lib/tauri", () => ({
  getProviderConfig: vi.fn().mockResolvedValue({
    generation: {
      provider: "sidecar",
      model_path: "models/llama-3.2-3b.gguf",
      external_url: null,
      api_key: null,
      model_name: null,
    },
  }),
  updateProvider: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../hooks/usePrivacyMode", () => ({
  usePrivacyMode: () => ({
    mode: privacyMock.mode,
    chosen: true,
    needs_migration_disclosure: false,
    ephemeral_disclosure_acknowledged: privacyMock.ephemeral_disclosure_acknowledged,
    loading: false,
    setMode: vi.fn(),
  }),
}));

// Mock the events module
vi.mock("../lib/events", () => {
  // The event functions return Promise<UnlistenFn>
  // UnlistenFn is a function that when called, unsubscribes
  const unlistenFn = vi.fn();
  return {
    onProviderLoading: vi.fn(() => Promise.resolve(unlistenFn)),
    onProviderReady: vi.fn(() => Promise.resolve(unlistenFn)),
    onProviderError: vi.fn(() => Promise.resolve(unlistenFn)),
  };
});

import { getProviderConfig, updateProvider } from "../lib/tauri";
import * as events from "../lib/events";

describe("GenerationPanel", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    privacyMock.mode = "ephemeral";
    privacyMock.ephemeral_disclosure_acknowledged = true;
    (getProviderConfig as ReturnType<typeof vi.fn>).mockResolvedValue({
      generation: {
        provider: "sidecar",
        model_path: "models/llama-3.2-3b.gguf",
        external_url: null,
        api_key: null,
        model_name: null,
      },
    });
    (updateProvider as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
  });

  it("switches from sidecar to external and calls updateProvider with correct payload", async () => {
    render(<GenerationPanel />);
    
    // Wait for config to load
    await waitFor(() => expect(getProviderConfig).toHaveBeenCalled());
    
    // Fill in external URL
    const urlInput = screen.getByPlaceholderText(/http:\/\/localhost/i);
    fireEvent.change(urlInput, { target: { value: "http://localhost:8080/v1" } });
    
    // Fill in API key (optional)
    const keyInput = screen.getByPlaceholderText(/sk-/i);
    fireEvent.change(keyInput, { target: { value: "test-key" } });
    
    // Click save
    const saveButton = screen.getByRole("button", { name: /Save/i });
    fireEvent.click(saveButton);
    
    await waitFor(() =>
      expect(updateProvider).toHaveBeenCalledWith(
        expect.objectContaining({
          provider: "external",
          external_url: "http://localhost:8080/v1",
          api_key: "test-key",
        })
      )
    );
  });

  it("maps ProviderNotReady to spinner, not error toast", async () => {
    // Mock provider-not-ready error
    (updateProvider as ReturnType<typeof vi.fn>).mockRejectedValue(
      new Error("provider-not-ready")
    );
    
    render(<GenerationPanel />);
    
    await waitFor(() => expect(getProviderConfig).toHaveBeenCalled());
    
    // Try to save with external URL
    const urlInput = screen.getByPlaceholderText(/http:\/\/localhost/i);
    fireEvent.change(urlInput, { target: { value: "http://localhost:8080/v1" } });
    
    const saveButton = screen.getByRole("button", { name: /Save/i });
    fireEvent.click(saveButton);
    
    // Should NOT show error toast/message for provider-not-ready
    await waitFor(() => {
      expect(screen.queryByText(/Failed to save/i)).not.toBeInTheDocument();
    });
  });

  it("shows red toast on settings save failure", async () => {
    (updateProvider as ReturnType<typeof vi.fn>).mockRejectedValue(
      new Error("disk write failed")
    );
    
    render(<GenerationPanel />);
    
    await waitFor(() => expect(getProviderConfig).toHaveBeenCalled());
    
    const urlInput = screen.getByPlaceholderText(/http:\/\/localhost/i);
    fireEvent.change(urlInput, { target: { value: "http://localhost:8080/v1" } });
    
    const saveButton = screen.getByRole("button", { name: /Save/i });
    fireEvent.click(saveButton);
    
    await waitFor(() => {
      expect(screen.getByText(/Failed to save settings to disk/i)).toBeInTheDocument();
    });
  });

  it("shows 'Waking up the Librarian' spinner during provider-loading", async () => {
    // Mock loading state by having onProviderLoading trigger immediately
    let loadingCallback: (() => void) | null = null;
    (events.onProviderLoading as ReturnType<typeof vi.fn>).mockImplementation((cb: () => void) => {
      loadingCallback = cb;
      return Promise.resolve(vi.fn());
    });
    
    render(<GenerationPanel />);
    
    // Simulate provider-loading event
    if (loadingCallback) {
      act(() => {
        loadingCallback!();
      });
    }
    
    expect(screen.getByText(/Waking up the Librarian/i)).toBeInTheDocument();
  });

  it("disables external URL fields in strict mode", async () => {
    privacyMock.mode = "strict";
    render(<GenerationPanel />);
    await waitFor(() => expect(getProviderConfig).toHaveBeenCalled());
    expect(screen.getByLabelText(/External base URL/i)).toBeDisabled();
    expect(screen.getByRole("button", { name: /Save/i })).toBeDisabled();
  });
});
