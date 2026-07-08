import {
  useProposalNotifications,
  setProposalNotificationsEnabled,
} from "../hooks/useProposalNotifications";
import { renderHook, act } from "@testing-library/react";
import { vi, describe, it, expect, beforeEach } from "vitest";

beforeEach(() => {
  setProposalNotificationsEnabled(false);
});

describe("useProposalNotifications", () => {
  it("returns enabled false by default", () => {
    const { result } = renderHook(() => useProposalNotifications());
    expect(result.current.enabled).toBe(false);
  });

  it("returns enabled true after enabling", () => {
    const { result } = renderHook(() => useProposalNotifications());
    act(() => {
      result.current.setEnabled(true);
    });
    expect(result.current.enabled).toBe(true);
  });

  it("persists across re-renders", () => {
    const { result, rerender } = renderHook(() => useProposalNotifications());
    act(() => {
      result.current.setEnabled(true);
    });
    rerender();
    expect(result.current.enabled).toBe(true);
  });
});
