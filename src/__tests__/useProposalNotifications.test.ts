import { renderHook } from "@testing-library/react";
import * as notificationPlugin from "@tauri-apps/plugin-notification";
import {
  useProposalNotifications,
  setProposalNotificationsEnabled,
} from "../hooks/useProposalNotifications";

vi.mock("@tauri-apps/plugin-notification");

describe("useProposalNotifications", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    vi.mocked(notificationPlugin.isPermissionGranted).mockResolvedValue(true);
    vi.mocked(notificationPlugin.sendNotification).mockResolvedValue(undefined);
  });

  test("when_toggle_on_and_queue_increases_sends_notification", async () => {
    setProposalNotificationsEnabled(true);

    const { rerender } = renderHook(
      ({ queueLength }) => useProposalNotifications(queueLength),
      { initialProps: { queueLength: 0 } }
    );

    // Simulate queue increasing from 0 to 2
    rerender({ queueLength: 2 });

    // Wait a tick for async operations
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(notificationPlugin.sendNotification).toHaveBeenCalledWith({
      title: "Curated Thoughts",
      body: "2 new proposals await review.",
    });
  });

  test("when_toggle_off_no_notification", async () => {
    setProposalNotificationsEnabled(false);

    const { rerender } = renderHook(
      ({ queueLength }) => useProposalNotifications(queueLength),
      { initialProps: { queueLength: 0 } }
    );

    // Simulate queue increasing
    rerender({ queueLength: 3 });

    // Wait a tick for async operations
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(notificationPlugin.sendNotification).not.toHaveBeenCalled();
  });

  test("on_first_load_no_notification", async () => {
    setProposalNotificationsEnabled(true);

    renderHook(
      ({ queueLength }) => useProposalNotifications(queueLength),
      { initialProps: { queueLength: 5 } }
    );

    // Wait a tick for async operations
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(notificationPlugin.sendNotification).not.toHaveBeenCalled();
  });

  test("when_queue_decreases_no_notification", async () => {
    setProposalNotificationsEnabled(true);

    const { rerender } = renderHook(
      ({ queueLength }) => useProposalNotifications(queueLength),
      { initialProps: { queueLength: 5 } }
    );

    // Simulate queue decreasing from 5 to 2
    rerender({ queueLength: 2 });

    // Wait a tick for async operations
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(notificationPlugin.sendNotification).not.toHaveBeenCalled();
  });

  test("delta_count_correct_in_message", async () => {
    setProposalNotificationsEnabled(true);

    const { rerender } = renderHook(
      ({ queueLength }) => useProposalNotifications(queueLength),
      { initialProps: { queueLength: 0 } }
    );

    // Test single proposal
    rerender({ queueLength: 1 });
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(notificationPlugin.sendNotification).toHaveBeenLastCalledWith({
      title: "Curated Thoughts",
      body: "1 new proposal awaits review.",
    });

    vi.clearAllMocks();

    // Test multiple proposals
    rerender({ queueLength: 4 });
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(notificationPlugin.sendNotification).toHaveBeenCalledWith({
      title: "Curated Thoughts",
      body: "3 new proposals await review.",
    });
  });
});
