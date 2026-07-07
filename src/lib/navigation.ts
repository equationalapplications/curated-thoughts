import { useCallback, useMemo, useState } from "react";
import type { AppMode } from "../components/shell/ModeRail";

export interface NavTarget {
  mode: AppMode;
  entityId?: string;
  docPath?: string;
}

export interface NavigationState {
  current: NavTarget;
  canGoBack: boolean;
  canGoForward: boolean;
  navigate: (target: NavTarget) => void;
  goBack: () => void;
  goForward: () => void;
}

interface Stacks {
  current: NavTarget;
  back: NavTarget[];
  forward: NavTarget[];
}

export function useNavigationState(
  initial: NavTarget = { mode: "brain" },
): NavigationState {
  const [state, setState] = useState<Stacks>({
    current: initial,
    back: [],
    forward: [],
  });

  const navigate = useCallback((target: NavTarget) => {
    setState((s) => ({
      current: target,
      back: [...s.back, s.current],
      forward: [],
    }));
  }, []);

  const goBack = useCallback(() => {
    setState((s) => {
      const prev = s.back.at(-1);
      if (!prev) return s;
      return {
        current: prev,
        back: s.back.slice(0, -1),
        forward: [s.current, ...s.forward],
      };
    });
  }, []);

  const goForward = useCallback(() => {
    setState((s) => {
      const next = s.forward[0];
      if (!next) return s;
      return {
        current: next,
        back: [...s.back, s.current],
        forward: s.forward.slice(1),
      };
    });
  }, []);

  return useMemo(
    () => ({
      current: state.current,
      canGoBack: state.back.length > 0,
      canGoForward: state.forward.length > 0,
      navigate,
      goBack,
      goForward,
    }),
    [state, navigate, goBack, goForward],
  );
}
