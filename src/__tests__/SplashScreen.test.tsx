import { screen, waitFor } from "@testing-library/react";
import { listen } from "@tauri-apps/api/event";
import { renderWithTheme } from "./test-utils";
import { SplashScreen } from "../components/shell/SplashScreen";

type Handler = (event: { payload: unknown }) => void;
let registeredHandlers: Record<string, Handler[]> = {};

beforeEach(() => {
  registeredHandlers = {};
  vi.mocked(listen).mockImplementation(async (event: string, handler: Handler) => {
    registeredHandlers[event] = registeredHandlers[event] ?? [];
    registeredHandlers[event].push(handler);
    return () => {};
  });
});

test("renders the splash message on mount", async () => {
  renderWithTheme(<SplashScreen onComplete={vi.fn()} />);
  expect(await screen.findByText(/Optimizing your library/i)).toBeInTheDocument();
});

test("progress bar reflects migration-progress events", async () => {
  renderWithTheme(<SplashScreen onComplete={vi.fn()} />);
  await screen.findByText(/Optimizing your library/i);
  // Fire a progress event with current=5, total=20.
  const handlers = registeredHandlers["migration-progress"];
  expect(handlers).toBeDefined();
  handlers[0]({ payload: { current: 5, total: 20, phase: "rechunk" } });
  await waitFor(() => {
    const bar = screen.getByRole("progressbar");
    expect(bar.getAttribute("aria-valuenow")).toBe("5");
    expect(bar.getAttribute("aria-valuemax")).toBe("20");
  });
});

test("calls onComplete when migration-complete fires", async () => {
  const onComplete = vi.fn();
  renderWithTheme(<SplashScreen onComplete={onComplete} />);
  await screen.findByText(/Optimizing your library/i);
  const handlers = registeredHandlers["migration-complete"];
  expect(handlers).toBeDefined();
  handlers[0]({ payload: undefined });
  await waitFor(() => expect(onComplete).toHaveBeenCalled());
});

test("renders error state with Restart to retry CTA on migration-error", async () => {
  renderWithTheme(<SplashScreen onComplete={vi.fn()} />);
  await screen.findByText(/Optimizing your library/i);
  const handlers = registeredHandlers["migration-error"];
  expect(handlers).toBeDefined();
  handlers[0]({ payload: { message: "kaboom" } });
  expect(await screen.findByText(/Migration failed/i)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /Restart to retry/i })).toBeInTheDocument();
});