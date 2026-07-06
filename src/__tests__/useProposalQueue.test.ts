import { describe, expect, it, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { useProposalQueue } from "../hooks/useProposalQueue";
import { makeProposalSummary } from "./fixtures/proposals";

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

describe("useProposalQueue", () => {
  it("loads pending proposals on mount", async () => {
    const proposals = [
      makeProposalSummary({
        id: "prop_1",
        target_name: "Alpha",
        created_at: 1,
      }),
    ];
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === "list_proposals_cmd") return Promise.resolve(proposals);
      return Promise.resolve(null);
    });

    const { result } = renderHook(() => useProposalQueue("/vault"));
    await waitFor(() => expect(result.current.queue).toEqual(proposals));
    expect(result.current.error).toBeNull();
    expect(invoke).toHaveBeenCalledWith("list_proposals_cmd", {
      filter: { status: "pending" },
    });
  });

  it("sets an error when list_proposals fails", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === "list_proposals_cmd") return Promise.reject(new Error("offline"));
      return Promise.resolve(null);
    });

    const { result } = renderHook(() => useProposalQueue("/vault"));
    await waitFor(() =>
      expect(result.current.error).toMatch(/temporarily unavailable/i),
    );
  });
});
