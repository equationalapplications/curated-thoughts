import { screen, fireEvent, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { PrivacyPanel } from "../components/settings/PrivacyPanel";
import { renderWithTheme } from "./test-utils";

test("mode 3 label is Connected agent with read-only disclosure", async () => {
  renderWithTheme(<PrivacyPanel />);
  expect(await screen.findByText(/Connected agent/i)).toBeInTheDocument();
  expect(screen.getByText(/nothing syncs/i)).toBeInTheDocument();
  expect(screen.queryByText(/Full cloud sync/i)).not.toBeInTheDocument();
});

test("Cloud Bridge panel disabled unless connected mode", async () => {
  renderWithTheme(<PrivacyPanel />);
  await screen.findByText(/Connected agent/i);
  expect(
    screen.getByText(/Cloud Bridge is only available in Connected agent privacy mode/i),
  ).toBeInTheDocument();
  expect(screen.getByLabelText(/Clanker pairing token/i)).toBeDisabled();
});

test("downgrade from connected to strict prompts confirm", async () => {
  vi.mocked(invoke).mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "get_privacy_mode") {
      return Promise.resolve({
        mode: "connected",
        chosen: true,
        needs_migration_disclosure: false,
        ephemeral_disclosure_acknowledged: true,
      });
    }
    if (cmd === "set_privacy_mode") {
      return Promise.resolve({
        disconnected_bridge: true,
        state: {
          mode: args?.mode,
          chosen: true,
          needs_migration_disclosure: false,
          ephemeral_disclosure_acknowledged: true,
        },
      });
    }
    if (cmd === "get_cloud_bridge_status") {
      return Promise.resolve({ configured: true, connection_status: "connected" });
    }
    return Promise.resolve(null);
  });

  const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
  renderWithTheme(<PrivacyPanel />);
  await screen.findByText(/Connected agent/i);
  fireEvent.click(screen.getByLabelText(/Strict \(default\)/i));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("set_privacy_mode", { mode: "strict" }),
  );
  expect(confirmSpy).toHaveBeenCalled();
  confirmSpy.mockRestore();
});
