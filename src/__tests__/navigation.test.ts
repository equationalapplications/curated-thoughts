import { renderHook, act } from "@testing-library/react";
import { useNavigationState } from "../lib/navigation";

test("starts on brain with empty history", () => {
  const { result } = renderHook(() => useNavigationState());
  expect(result.current.current).toEqual({ mode: "brain" });
  expect(result.current.canGoBack).toBe(false);
  expect(result.current.canGoForward).toBe(false);
});

test("navigate pushes history; back and forward walk it", () => {
  const { result } = renderHook(() => useNavigationState());

  act(() => result.current.navigate({ mode: "library", docPath: "documents/a.md" }));
  act(() => result.current.navigate({ mode: "brain", entityId: "ent_1" }));
  expect(result.current.canGoBack).toBe(true);

  act(() => result.current.goBack());
  expect(result.current.current).toEqual({ mode: "library", docPath: "documents/a.md" });
  expect(result.current.canGoForward).toBe(true);

  act(() => result.current.goForward());
  expect(result.current.current).toEqual({ mode: "brain", entityId: "ent_1" });
  expect(result.current.canGoForward).toBe(false);
});

test("navigate clears the forward stack", () => {
  const { result } = renderHook(() => useNavigationState());
  act(() => result.current.navigate({ mode: "review" }));
  act(() => result.current.goBack());
  act(() => result.current.navigate({ mode: "settings" }));
  expect(result.current.canGoForward).toBe(false);
});

test("goBack and goForward are no-ops at the stack edges", () => {
  const { result } = renderHook(() => useNavigationState());
  act(() => result.current.goBack());
  act(() => result.current.goForward());
  expect(result.current.current).toEqual({ mode: "brain" });
});
