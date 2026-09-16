import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { message, open } from "@tauri-apps/plugin-dialog";
import { useVaultSwitcher } from "../hooks/useVaultSwitcher";
import {
  backupVaultDb,
  checkVaultBackup,
  switchVault,
} from "../lib/tauri";

vi.mock("../hooks/useWikiStatus", () => ({
  useWikiStatus: () => ({ busy: false, activeJobLabel: null }),
}));

vi.mock("../lib/tauri", () => ({
  backupVaultDb: vi.fn().mockResolvedValue("/old/.brain/brain.db.bak"),
  checkVaultBackup: vi.fn().mockResolvedValue(false),
  switchVault: vi.fn().mockResolvedValue(undefined),
}));

const asMock = (fn: unknown) => fn as ReturnType<typeof vi.fn>;

/** Every message() call's body text, in order. */
const bodies = () => asMock(message).mock.calls.map((c) => String(c[0]));
/** Every message() call's options, in order. */
const opts = () =>
  asMock(message).mock.calls.map((c) => (c[1] ?? {}) as Record<string, unknown>);

beforeEach(() => {
  vi.clearAllMocks();
  asMock(open).mockResolvedValue("/new/vault");
  asMock(checkVaultBackup).mockResolvedValue(false);
});

describe("useVaultSwitcher", () => {
  it("names the knowledge base in the backup prompt, not just the index", async () => {
    asMock(message).mockResolvedValue("Cancel");
    const { result } = renderHook(() => useVaultSwitcher("/old/vault"));

    await act(async () => {
      await result.current.changeVault();
    });

    expect(bodies()[0]).toMatch(/knowledge base/i);
    expect(bodies()[0]).toMatch(/agent memories/i);
    expect(bodies()[0]).not.toMatch(/your indexed data/i);
  });

  it("gates 'continue without backup' behind a destructive warning", async () => {
    // "No" = Continue without backup; then "No" = Go back.
    asMock(message).mockResolvedValue("No");
    const { result } = renderHook(() => useVaultSwitcher("/old/vault"));

    await act(async () => {
      await result.current.changeVault();
    });

    const gate = opts()[1];
    expect(gate.title).toBe("Destroy this vault's knowledge?");
    expect(gate.kind).toBe("warning");
    expect(bodies()[1]).toMatch(/cannot be undone/i);
    expect(switchVault).not.toHaveBeenCalled();
  });

  it("fires the gate even when the target vault has a backup, before the restore prompt", async () => {
    asMock(checkVaultBackup).mockResolvedValue(true);
    asMock(message).mockResolvedValue("No");
    const { result } = renderHook(() => useVaultSwitcher("/old/vault"));

    await act(async () => {
      await result.current.changeVault();
    });

    // Exactly two dialogs: backup prompt, then the gate. The restore prompt
    // must not have been reached, because the gate returned "Go back".
    expect(asMock(message).mock.calls).toHaveLength(2);
    expect(opts()[1].title).toBe("Destroy this vault's knowledge?");
    expect(switchVault).not.toHaveBeenCalled();
  });

  it("switches when the user confirms the destructive gate", async () => {
    asMock(message)
      .mockResolvedValueOnce("No") // continue without backup
      .mockResolvedValueOnce("Yes"); // switch without backup
    const { result } = renderHook(() => useVaultSwitcher("/old/vault"));

    await act(async () => {
      await result.current.changeVault();
    });

    expect(switchVault).toHaveBeenCalledWith("/new/vault", false);
    expect(backupVaultDb).not.toHaveBeenCalled();
  });

  it("does not gate when the user takes a backup", async () => {
    asMock(message).mockResolvedValue("Yes");
    const { result } = renderHook(() => useVaultSwitcher("/old/vault"));

    await act(async () => {
      await result.current.changeVault();
    });

    expect(backupVaultDb).toHaveBeenCalled();
    expect(
      opts().some((o) => o.title === "Destroy this vault's knowledge?"),
    ).toBe(false);
    expect(switchVault).toHaveBeenCalled();
  });

  it("says a restore brings back the knowledge base too", async () => {
    asMock(checkVaultBackup).mockResolvedValue(true);
    asMock(message).mockResolvedValue("Yes");
    const { result } = renderHook(() => useVaultSwitcher("/old/vault"));

    await act(async () => {
      await result.current.changeVault();
    });

    const restorePrompt = bodies().find((b) => /restore/i.test(b)) ?? "";
    expect(restorePrompt).toMatch(/knowledge base/i);
    expect(restorePrompt).toMatch(/re-indexed/i);
  });
});
